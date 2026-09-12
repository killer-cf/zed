use std::{
    collections::BTreeSet,
    net::IpAddr,
    num::NonZeroU16,
    sync::Arc,
};

use anyhow::{Context as _, Result, ensure};
use base64::{
    Engine as _,
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
};
use credentials_provider::CredentialsProvider;
use db::kvp::KeyValueStore;
use ed25519_dalek::{SigningKey, VerifyingKey};
use gpui::AsyncApp;
use mobile_protocol::Capability;
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq;
use time::OffsetDateTime;
use uuid::Uuid;
use zeroize::Zeroizing;

pub const MOBILE_CREDENTIALS_URL: &str = "zed://mobile-server";
pub const MOBILE_HOST_KEY_USERNAME: &str = "host-signing-key";

const MOBILE_KVP_BINDING_KEY: &str = "mobile_server/v1/binding";
const MOBILE_KVP_GRANTS_KEY: &str = "mobile_server/v1/grants";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MobileBinding {
    pub enabled: bool,
    pub address: IpAddr,
    pub port: NonZeroU16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceGrant {
    pub id: Uuid,
    pub label: String,
    pub client_public_key: String,
    pub token_digest: String,
    pub capabilities: BTreeSet<Capability>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<OffsetDateTime>,
}

pub struct MobileStore {
    key_value_store: KeyValueStore,
    credentials_provider: Arc<dyn CredentialsProvider>,
}

impl MobileStore {
    pub fn new(
        key_value_store: KeyValueStore,
        credentials_provider: Arc<dyn CredentialsProvider>,
    ) -> Self {
        Self {
            key_value_store,
            credentials_provider,
        }
    }

    pub async fn load_or_create_host_key(&self, cx: &AsyncApp) -> Result<SigningKey> {
        if let Some((username, password)) = self
            .credentials_provider
            .read_credentials(MOBILE_CREDENTIALS_URL, cx)
            .await
            .context("reading mobile host signing key")?
        {
            let password = Zeroizing::new(password);
            ensure!(
                username == MOBILE_HOST_KEY_USERNAME,
                "mobile host credential has an unexpected username"
            );
            ensure!(
                password.len() == 32,
                "mobile host credential must contain exactly 32 bytes"
            );
            let secret_bytes = Zeroizing::new(
                <[u8; 32]>::try_from(password.as_slice())
                    .map_err(|_| anyhow::anyhow!("invalid mobile host credential length"))?,
            );
            return Ok(SigningKey::from_bytes(&secret_bytes));
        }

        let signing_key = SigningKey::generate(&mut OsRng);
        let secret_bytes = Zeroizing::new(signing_key.to_bytes());
        self.credentials_provider
            .write_credentials(
                MOBILE_CREDENTIALS_URL,
                MOBILE_HOST_KEY_USERNAME,
                secret_bytes.as_slice(),
                cx,
            )
            .await
            .context("writing mobile host signing key")?;
        Ok(signing_key)
    }

    pub async fn save_binding(&self, binding: MobileBinding) -> Result<()> {
        validate_binding(&binding)?;
        let value = serde_json::to_string(&binding).context("serializing mobile binding")?;
        self.key_value_store
            .write_kvp(MOBILE_KVP_BINDING_KEY.to_owned(), value)
            .await
            .context("persisting mobile binding")
    }

    pub async fn load_binding(&self) -> Result<Option<MobileBinding>> {
        let Some(value) = self
            .key_value_store
            .read_kvp(MOBILE_KVP_BINDING_KEY)
            .context("reading mobile binding")?
        else {
            return Ok(None);
        };
        let binding = serde_json::from_str(&value).context("decoding mobile binding")?;
        validate_binding(&binding)?;
        Ok(Some(binding))
    }

    pub async fn insert_grant(&self, grant: DeviceGrant) -> Result<()> {
        validate_grant(&grant)?;
        let mut grants = self.read_grants()?;
        ensure!(
            grants.iter().all(|existing| existing.id != grant.id),
            "a mobile grant with this id already exists"
        );
        grants.push(grant);
        self.write_grants(grants).await
    }

    pub async fn grant(&self, id: Uuid) -> Result<Option<DeviceGrant>> {
        Ok(self
            .read_grants()?
            .into_iter()
            .find(|grant| grant.id == id))
    }

    pub async fn grants(&self) -> Result<Vec<DeviceGrant>> {
        self.read_grants()
    }

    pub async fn revoke_grant(&self, id: Uuid, at: OffsetDateTime) -> Result<bool> {
        let mut grants = self.read_grants()?;
        let Some(grant) = grants.iter_mut().find(|grant| grant.id == id) else {
            return Ok(false);
        };
        if grant.revoked_at.is_some() {
            return Ok(false);
        }
        grant.revoked_at = Some(at);
        validate_grant(grant)?;
        self.write_grants(grants).await?;
        Ok(true)
    }

    fn read_grants(&self) -> Result<Vec<DeviceGrant>> {
        let Some(value) = self
            .key_value_store
            .read_kvp(MOBILE_KVP_GRANTS_KEY)
            .context("reading mobile grants")?
        else {
            return Ok(Vec::new());
        };
        let grants: Vec<DeviceGrant> =
            serde_json::from_str(&value).context("decoding mobile grants")?;
        let mut ids = BTreeSet::new();
        for grant in &grants {
            validate_grant(grant)?;
            ensure!(
                ids.insert(grant.id),
                "mobile grant list contains a duplicate id"
            );
        }
        Ok(grants)
    }

    async fn write_grants(&self, grants: Vec<DeviceGrant>) -> Result<()> {
        let value = serde_json::to_string(&grants).context("serializing mobile grants")?;
        self.key_value_store
            .write_kvp(MOBILE_KVP_GRANTS_KEY.to_owned(), value)
            .await
            .context("persisting mobile grants")
    }
}

pub fn token_digest(token: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token))
}

pub fn token_matches(stored_digest: &str, token: &[u8]) -> bool {
    let Ok(stored_digest) = decode_base64_32(stored_digest, "token digest") else {
        return false;
    };
    let computed_digest = Sha256::digest(token);
    computed_digest
        .as_slice()
        .ct_eq(stored_digest.as_slice())
        .into()
}

fn validate_binding(binding: &MobileBinding) -> Result<()> {
    ensure!(binding.port.get() != 0, "mobile binding port must be nonzero");
    Ok(())
}

fn validate_grant(grant: &DeviceGrant) -> Result<()> {
    ensure!(!grant.label.is_empty(), "mobile grant label must not be empty");
    ensure!(
        grant.label.len() <= 128,
        "mobile grant label must be at most 128 bytes"
    );

    let client_public_key = decode_base64_32(&grant.client_public_key, "client public key")?;
    VerifyingKey::from_bytes(&client_public_key).context("invalid mobile client public key")?;
    decode_base64_32(&grant.token_digest, "token digest")?;
    Ok(())
}

fn decode_base64_32(value: &str, field: &str) -> Result<Zeroizing<[u8; 32]>> {
    let decoded = Zeroizing::new(
        URL_SAFE
            .decode(value)
            .or_else(|_| URL_SAFE_NO_PAD.decode(value))
            .with_context(|| format!("{field} is not valid base64url"))?,
    );
    ensure!(
        decoded.len() == 32,
        "{field} must decode to exactly 32 bytes"
    );
    let mut bytes = Zeroizing::new([0; 32]);
    bytes.copy_from_slice(&decoded);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::{
        future::Future,
        net::IpAddr,
        num::NonZeroU16,
        pin::Pin,
        sync::{Arc, Mutex},
    };

    use anyhow::Result;
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use credentials_provider::CredentialsProvider;
    use db::kvp::KeyValueStore;
    use ed25519_dalek::SigningKey;
    use gpui::AsyncApp;
    use mobile_protocol::Capability;
    use time::OffsetDateTime;
    use uuid::Uuid;
    use super::{
        DeviceGrant, MobileBinding, MobileStore, MOBILE_CREDENTIALS_URL,
        MOBILE_HOST_KEY_USERNAME, token_digest, token_matches,
    };

    #[derive(Default)]
    struct RecordingCredentialsProvider {
        credentials: Mutex<Option<(String, Vec<u8>)>>,
    }

    impl CredentialsProvider for RecordingCredentialsProvider {
        fn read_credentials<'a>(
            &'a self,
            _url: &'a str,
            _cx: &'a AsyncApp,
        ) -> Pin<Box<dyn Future<Output = Result<Option<(String, Vec<u8>)>>> + 'a>> {
            let credentials = self
                .credentials
                .lock()
                .expect("credential lock is not poisoned")
                .clone();
            Box::pin(async move { Ok(credentials) })
        }

        fn write_credentials<'a>(
            &'a self,
            _url: &'a str,
            _username: &'a str,
            password: &'a [u8],
            _cx: &'a AsyncApp,
        ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
            let password = password.to_vec();
            let credentials = &self.credentials;
            Box::pin(async move {
                *credentials
                    .lock()
                    .expect("credential lock is not poisoned") =
                    Some((MOBILE_HOST_KEY_USERNAME.to_owned(), password));
                Ok(())
            })
        }

        fn delete_credentials<'a>(
            &'a self,
            _url: &'a str,
            _cx: &'a AsyncApp,
        ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }

    fn test_grant(token: &[u8]) -> DeviceGrant {
        let client_signing_key = SigningKey::from_bytes(&[7; 32]);
        DeviceGrant {
            id: Uuid::from_u128(1),
            label: "Test phone".to_owned(),
            client_public_key: URL_SAFE_NO_PAD
                .encode(client_signing_key.verifying_key().to_bytes()),
            token_digest: token_digest(token),
            capabilities: [Capability::StatusRead].into_iter().collect(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            revoked_at: None,
        }
    }

    #[gpui::test]
    async fn persists_host_identity_and_revocable_grants(cx: &gpui::TestAppContext) {
        let database = KeyValueStore::open_test_db("mobile_store_identity_and_grants").await;
        let credentials_provider = Arc::new(RecordingCredentialsProvider::default());
        let store = MobileStore::new(database, credentials_provider.clone());
        let async_cx = cx.to_async();

        let first_key = store
            .load_or_create_host_key(&async_cx)
            .await
            .expect("host key should load");
        let second_key = store
            .load_or_create_host_key(&async_cx)
            .await
            .expect("host key should load");
        assert_eq!(first_key.verifying_key(), second_key.verifying_key());
        assert_ne!(first_key.to_bytes(), [0; 32]);

        let binding = MobileBinding {
            enabled: true,
            address: "100.88.4.2".parse::<IpAddr>().expect("test address is valid"),
            port: NonZeroU16::new(6769).expect("test port is nonzero"),
        };
        store
            .save_binding(binding.clone())
            .await
            .expect("binding should save");
        assert_eq!(
            store.load_binding().await.expect("binding should load"),
            Some(binding)
        );

        let grant = test_grant(b"test-device-token");
        assert!(token_matches(&grant.token_digest, b"test-device-token"));
        assert!(!token_matches(&grant.token_digest, b"wrong-token"));
        store
            .insert_grant(grant.clone())
            .await
            .expect("grant should save");
        assert_eq!(
            store.grants().await.expect("grants should load"),
            vec![grant.clone()]
        );
        assert!(store
            .revoke_grant(grant.id, OffsetDateTime::now_utc())
            .await
            .expect("grant should revoke"));
        assert!(store
            .grant(grant.id)
            .await
            .expect("grant lookup should succeed")
            .expect("grant exists")
            .revoked_at
            .is_some());

        let stored_credentials = credentials_provider
            .credentials
            .lock()
            .expect("credential lock is not poisoned")
            .clone()
            .expect("host key should be persisted");
        assert_eq!(stored_credentials.0, MOBILE_HOST_KEY_USERNAME);
        assert_eq!(stored_credentials.1.len(), 32);
        assert_eq!(MOBILE_CREDENTIALS_URL, "zed://mobile-server");

        let grants_json = store
            .key_value_store
            .read_kvp("mobile_server/v1/grants")
            .expect("grant list should read")
            .expect("grant list should be persisted");
        assert!(!grants_json.contains(&URL_SAFE_NO_PAD.encode(first_key.to_bytes())));
        assert!(!grants_json.contains("test-device-token"));
    }

    #[gpui::test]
    async fn rejects_invalid_grant_before_persisting(_cx: &gpui::TestAppContext) {
        let database = KeyValueStore::open_test_db("mobile_store_invalid_grant").await;
        let store = MobileStore::new(
            database,
            Arc::new(RecordingCredentialsProvider::default()),
        );
        let mut invalid = test_grant(b"test-device-token");
        invalid.client_public_key = "not-base64".to_owned();

        assert!(store.insert_grant(invalid).await.is_err());
        assert!(store.grants().await.expect("grants should load").is_empty());
    }

    #[gpui::test]
    async fn rejects_corrupt_grant_without_returning_it(cx: &gpui::TestAppContext) {
        let database = KeyValueStore::open_test_db("mobile_store_corrupt_grant").await;
        let store = MobileStore::new(
            database,
            Arc::new(RecordingCredentialsProvider::default()),
        );
        let malformed = serde_json::json!([{
            "id": Uuid::from_u128(2),
            "label": "Corrupt",
            "client_public_key": "not-base64",
            "token_digest": URL_SAFE_NO_PAD.encode([1; 32]),
            "capabilities": ["status_read"],
            "created_at": "1970-01-01T00:00:00Z",
            "revoked_at": null
        }]);
        store
            .key_value_store
            .write_kvp(
                "mobile_server/v1/grants".to_owned(),
                serde_json::to_string(&malformed).expect("malformed fixture should serialize"),
            )
            .await
            .expect("malformed fixture should save");

        assert!(store.grants().await.is_err());
        assert!(store.grant(Uuid::from_u128(2)).await.is_err());
        let _ = cx;
    }

    #[gpui::test]
    async fn rejects_malformed_host_secret(cx: &gpui::TestAppContext) {
        let database = KeyValueStore::open_test_db("mobile_store_corrupt_host_key").await;
        let credentials_provider = Arc::new(RecordingCredentialsProvider {
            credentials: Mutex::new(Some((
                MOBILE_HOST_KEY_USERNAME.to_owned(),
                vec![1; 31],
            ))),
        });
        let store = MobileStore::new(database, credentials_provider);

        assert!(store.load_or_create_host_key(&cx.to_async()).await.is_err());
    }

}
