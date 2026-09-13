# Companion mobile do Zed via Tailscale

**Status:** Design aprovado pelo usuário.

## Objetivo

Criar um aplicativo iOS/Android que permita operar remotamente o Zed que permanece em execução em um host conectado ao mesmo tailnet Tailscale. O aplicativo terá paridade funcional com o Orca Mobile: hosts pareados, worktrees, threads de agentes e terminal, chat e prompts, Git, arquivos, criação de workspaces, comandos rápidos, contas e notificações.

O computador que executa Zed é sempre a fonte de verdade para repositórios, worktrees, processos, sessões, credenciais e estado do Git. O aplicativo não executa agentes e nunca replica o repositório para o telefone.

## Escopo

- Um aplicativo React Native/Expo para iOS e Android, integrado ao mesmo repositório do Zed.
- Um servidor Mobile RPC local do Zed, acessível exclusivamente por um endereço Tailscale escolhido pelo usuário.
- Pareamento por QR code de uso único e concessões independentes, revogáveis por dispositivo.
- Controle integral de Agent Threads ACP e Terminal Threads do Agent Panel, incluindo sessões OMP persistidas.
- Navegação por worktrees, árvore de arquivos, alterações Git, staging, commit e criação de workspace/worktree.
- Visualização de terminal e Chat UI, entrada remota, prompts, permissões, anexos, teclas especiais, comandos rápidos, parar, retomar e recuperar sessões.
- Status de agente, contas e uso quando o provedor do host expuser esses dados; recursos indisponíveis são anunciados como capabilities, nunca simulados.
- Notificações móveis para eventos de thread, conclusão de agente e pedidos de atenção.
- Reconexão previsível ao voltar do background, ao recuperar conectividade e após a interrupção de uma rota Tailscale.

Fora de escopo: publicar uma porta do Zed na Internet, parear ou autenticar dispositivos no Tailscale, executar agentes no telefone, editar arquivos como um IDE móvel completo e manter um serviço Relay de terceiros. A primeira versão exige que o host e o telefone já estejam autenticados no mesmo tailnet.

## Referência Orca e licença

O Orca Mobile usa React Native/Expo, QR para uma oferta de pareamento, WebSocket RPC versionado, tokens de dispositivo e a chave pública fixa do host. Seu repositório está sob MIT. Componentes de interface ou estrutura de navegação podem ser reutilizados seletivamente, desde que os avisos de copyright e a licença MIT sejam preservados onde houver cópia substancial.

O Zed não reutiliza o protocolo, a marca, o esquema `orca://`, ícones ou assets do Orca. O app mantém uma linguagem visual comparável — tema escuro, cards de host/worktree, sessão de retomada e ações rápidas — mas usa identidade, assets e esquemas de URL próprios do Zed. O contrato RPC será específico para os tipos e as regras de segurança do Zed.

## Arquitetura

```text
Zed desktop (host Tailscale)
  ├─ crate mobile_protocol
  │   └─ contrato JSON versionado, sem dependência de domínio/UI
  ├─ crate mobile_server
  │   ├─ oferta QR e grants de dispositivos
  │   ├─ handshake autenticado e conexão WebSocket
  │   ├─ negociação de versão e capabilities
  │   ├─ janela nativa Mobile e ação própria para abri-la
  │   └─ encaminhamento de RPC e subscriptions
  ├─ crates adaptadores de domínio
  │   ├─ AgentThread / ConversationView
  │   ├─ TerminalThread / terminal vivo / sessão OMP persistida
  │   ├─ Project / worktree / árvore de arquivos
  │   ├─ Git: status, diff, stage, unstage e commit
  │   └─ agent accounts, quick commands e notificações
  └─ armazenamento local de grants e chave do host
                 │
                 │ WebSocket autenticado sobre rota Tailscale
                 ▼
Zed Mobile (iOS/Android)
  ├─ armazenamento seguro: token, identidade do dispositivo e chave do host
  ├─ cliente RPC, heartbeat, reconexão e controle de compatibilidade
  ├─ hosts, worktrees, threads, terminal, Chat UI e Git
  └─ notificações e estado por host
```

`MobileServer` é uma fronteira assíncrona. Ele recebe mensagens de rede fora da thread de UI e agenda operações de domínio no contexto GPUI apropriado; entidades GPUI nunca cruzam a fronteira de thread. Cada operação devolve resultado estruturado e cada stream é cancelado quando o app ou a sessão remota é fechado.

### Fronteira para atualizações upstream

O companion fica em crates e diretório próprios: `mobile_protocol`, `mobile_server`, adaptadores móveis específicos de domínio e `mobile/`. A integração inicial no Zed fica limitada a registrar e inicializar `mobile_server` em um ponto do startup; a janela de controle, QR, configuração e grants pertencem ao novo crate e não alteram `settings_ui`.

Os planos posteriores consomem APIs públicas existentes. Quando uma API atual não expuser o dado mínimo, a alteração no crate original deve ser um método público aditivo ou uma projeção DTO estreita, com teste focado no mesmo arquivo. Não são permitidos refactors amplos, mudanças de semântica existente, mover lógica de domínio para o companion ou editar arquivos originais apenas por estilo. A consequência é reduzir conflitos de merge e permitir atualizar o Zed upstream sem carregar um fork de `agent_ui`, `terminal`, `project`, `git` ou `settings_ui`.

O servidor é iniciado somente após o usuário habilitar Mobile no Zed e selecionar um endereço Tailscale. Ele não aceita `127.0.0.1` como endereço anunciável para outro dispositivo, endereços wildcard, LAN comum, Relay ou interfaces públicas. A porta é estável por host, configurada quando Mobile é habilitado e mantida nos perfis pareados; mudar a porta exige atualizar o endpoint salvo ou parear novamente. Não é atribuída aleatoriamente, pois o aplicativo precisa reconectar após o host reiniciar.

## Pareamento e autorização

1. O host gera uma oferta de uso único com identificador aleatório, validade curta, endpoint Tailscale, versão de protocolo, chave pública persistente do host e segredo de pareamento.
2. O usuário exibe o QR em Zed. O QR pode ser escaneado ou colado como código/deep link no aplicativo.
3. O aplicativo gera uma identidade de dispositivo e a guarda no armazenamento seguro do sistema. Ele abre a rota Tailscale e confirma a chave pública do host que estava fixada na oferta.
4. A troca de pareamento consome o segredo uma única vez, vincula a chave do dispositivo a um novo grant revogável e devolve um token opaco específico daquele dispositivo.
5. Conexões posteriores apresentam o token e assinam um desafio com a chave do dispositivo; o host também prova posse da chave fixada. O grant determina as capabilities concedidas.
6. O painel Mobile do Zed lista cada dispositivo pareado. Revogar um grant encerra streams e invalida conexões novas imediatamente. Gerar nova oferta invalida somente ofertas ainda não consumidas.

Tailscale protege o transporte da rota privada, mas não substitui autenticação da aplicação. O QR e seu segredo são credenciais: não entram em logs, telemetria, títulos de janela ou histórico; expiram; não podem ser reutilizados; e nunca são aceitos por uma porta exposta publicamente.

## Contrato Mobile RPC

O protocolo é versionado e separado da implementação UI. `status.get` é a primeira chamada autenticada e informa a versão do host, a versão mínima aceita, o conjunto de capabilities, o host e o estado de conectividade. O app bloqueia um host incompatível com uma tela de atualização explícita, em vez de interpretar silenciosamente payloads novos ou antigos.

As mensagens usam envelopes tipados de requisição, resultado, erro e evento. Métodos mutantes levam um ID de operação idempotente para que uma reconexão não duplique `send`, `stage`, `commit`, `stop`, `resume` ou criação de workspace. Erros identificam ao menos: host inacessível, sessão revogada, key mismatch, oferta expirada, versão bloqueada, recurso indisponível, entrada não permitida e estado obsoleto.

As capabilities começam por estes grupos:

- `threads.read`, `threads.control`, `terminal.stream`, `terminal.input`, `chat.send` e `attachments.send`.
- `worktrees.read`, `worktrees.create`, `files.read` e `source_control.write`.
- `quick_commands`, `accounts.read`, `accounts.switch` e `usage.read`.
- `notifications` e `browser.mobile_view`.

O cliente oculta ou desabilita ações sem capability. O host é autoritativo em toda autorização e valida o estado da thread/worktree no momento da operação.

## Modelo de threads e dados ao vivo

O catálogo de threads reúne dois conjuntos existentes sem fundi-los artificialmente:

- `ThreadMetadataStore` identifica Agent Threads ACP, seus IDs duráveis, agentes, sessões e worktrees.
- `TerminalThreadMetadataStore` identifica Terminal Threads e preserva uma `TerminalAgentSession` OMP quando a sessão for retomável.

Metadata persistida serve para catálogo e retomada; ela não é uma fonte de scrollback nem uma autorização de controle. O MobileServer obtém o estado vivo das entidades do Agent Panel e do terminal, publica snapshots mínimos e segue com eventos incrementais. Uma seleção de uma thread persistida ainda deve restaurá-la apenas quando o usuário abre a sessão, como no desktop; listar a thread pelo telefone nunca inicia uma sessão automaticamente.

A visão de Thread contém título, worktree, tipo, agente, sessão, status e ações permitidas. Ela expõe streams de transcript/terminal e métodos para abrir, enviar mensagem, enviar entrada terminal, responder prompt ou permissão, interromper, retomar, fechar e criar uma sessão relacionada. Ações que possam afetar um processo usam a mesma semântica do desktop e são atribuídas ao grant do dispositivo no log local de ações.

A viewport do terminal é parte da assinatura para que o host não envie um snapshot custoso e inútil. O app hidrata scrollback recente, aplica deltas em ordem e pede novo snapshot após uma lacuna. A entrada padrão é uma composição que exige envio explícito; o modo Live, com keystrokes imediatos, é uma opção separada e visível. A linha de acessórios traz ao menos Tab, Shift+Tab, Escape, Ctrl+C e Enter.

## Experiência móvel

A tela inicial apresenta hosts pareados, conexão, contagem e estado de worktrees, uma sessão que possa ser retomada, uso de conta disponível e ações para parear host ou criar workspace. Cada host abre uma lista pesquisável de worktrees com estado de agente: trabalhando, concluído, esperando entrada, interrompido ou indisponível.

A tela de worktree oferece abas de sessões. Uma sessão suportada abre por padrão no modo configurado pelo dispositivo: Terminal ou Chat UI; o usuário pode alternar só aquela aba. Chat UI mostra transcript, composer, perguntas/permissões e anexos. Terminal mantém seleção/cópia/colar, scrollback, comandos rápidos e entrada Live. O aplicativo não apresenta essas vistas como um editor de código.

A área Source Control mostra arquivos alterados, diffs, stage, unstage e commit. A árvore de arquivos é somente leitura e é acessada pela worktree para contexto e inspeção. Criar workspace seleciona host e fonte suportada pelo Zed; o host cria e notifica a nova worktree em vez de o telefone criar estado local.

Contas, uso e Quick Commands refletem dados do host e somente aparecem onde a capability existir. Notificações levam ao host, worktree e thread corretos, mas não contêm tokens, conteúdo de prompt ou saída sensível enquanto a tela estiver bloqueada.

## Conectividade e falhas

O estado de conexão é explícito: `connected`, `reconnecting`, `unreachable`, `revoked`, `incompatible` ou `key_mismatch`. Um heartbeat detecta sockets half-open. Ao voltar para foreground, mudar de rede ou recuperar uma rota Tailscale, o cliente executa uma prova de vida imediata e reinicia a reconexão, em vez de permanecer indefinidamente em backoff esgotado. A sessão expõe `Retry` em qualquer tela que permita ação, não apenas na lista de hosts.

Falha de pareamento mostra o estágio e um diagnóstico seguro: QR inválido, câmera negada com alternativa de colar código, oferta expirada, host não alcançável, Tailscale desconectado, chave do host divergente ou grant revogado. O aplicativo não tenta substituir Tailscale, mudar endpoint sem ação do usuário, aceitar uma chave nova silenciosamente ou reenviar uma mutação após resultado ambíguo.

Quando o desktop encerra, o app preserva o perfil do host e seus dados não sensíveis em cache, mostra-o como offline e reconecta quando o host voltar. Revogação limpa token e identidade daquele host do armazenamento seguro.

## Entrega por épicos

A paridade completa é obrigatória, mas cada épico mantém uma fronteira testável e entregável:

1. Fundação: configuração Mobile no Zed, MobileServer, QR, grants, armazenamento seguro, versão, capabilities e reconexão.
2. Controle de threads: catálogo de Agent/Terminal Threads, status, streams, Chat UI/terminal, entrada, prompts, stop, resume e recuperação OMP.
3. Worktrees e código: worktrees, arquivos, criação de workspace, Git, diffs, stage, unstage e commit.
4. Operação completa: Quick Commands, contas/uso quando disponíveis, anexos, browser mobile view, notificações e configurações por dispositivo.
5. Paridade de qualidade: acessibilidade, estados offline, reconexão em background, compatibilidade de versões, segurança de grants e validação em dispositivos iOS/Android.

Nenhum épico introduz uma API pública ou atalho de rede que seja necessário manter depois. Features móveis dependem de capabilities explícitas enquanto o suporte correspondente não estiver instalado no host.

## Verificação de aceitação

1. Um host Tailscale habilitado gera QR de uso único; app pareia por scan ou paste e reconecta com uma identidade de dispositivo persistida em armazenamento seguro.
2. Oferta expirada, QR inválido, host fora do tailnet, chave divergente e grant revogado resultam em estados distintos e não concedem acesso.
3. Revogar um dispositivo encerra seus streams, impede nova conexão e não afeta grants de outros dispositivos.
4. Um telefone lista Agent Threads ACP e Terminal Threads OMP sem iniciar sessões persistidas. Abrir explicitamente uma sessão OMP retomável usa o fluxo de retomada já existente.
5. O usuário vê saída ao vivo e pode enviar prompt, entrada, teclas especiais, resposta a permissão, stop e resume. Repetir uma mutação após perda de conexão não duplica o efeito.
6. O telefone pode navegar worktrees e arquivos, revisar diff, stage, unstage, commit e criar workspace no host que possui o repositório.
7. Recursos de contas, uso, comandos, anexos, browser e notificações são operacionais onde anunciados e corretamente indisponíveis onde não anunciados.
8. Deixar o app em background, matar a rota e voltar ao foreground não deixa uma sessão falsamente conectada: ela prova vida, reconecta ou exibe ação de retry.
9. Versões incompatíveis bloqueiam o host antes de qualquer operação de domínio.
10. O MobileServer nunca anuncia nem aceita a rota pública/LAN, nem registra QR, token ou segredo de pareamento.

## Referências

- [Orca Mobile README](https://github.com/stablyai/orca/blob/main/mobile/README.md)
- [Orca Mobile companion](https://www.onorca.dev/docs/mobile)
- [Orca Remote Servers](https://www.onorca.dev/docs/remote-servers)
- [Orca: diagnóstico de reconexão Android](https://github.com/stablyai/orca/blob/main/mobile/issue-5049-unresponsive-session-findings.md)
- [Licença MIT do Orca](https://github.com/stablyai/orca/blob/main/LICENSE)
