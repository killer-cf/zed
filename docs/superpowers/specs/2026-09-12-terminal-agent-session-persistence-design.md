# Persistência de sessões de agentes em Terminal Threads

**Status:** Design aprovado pelo usuário.

## Objetivo

Associar automaticamente uma Terminal Thread local do Agent Panel à sessão persistida do agente que estiver executando nela. Depois de fechar o Zed ou reiniciar a máquina, selecionar a mesma thread inicia um shell local novo no diretório da sessão e retoma o alvo correto. A primeira implementação suporta somente OMP: `omp --resume '<session_id>'`.

## Escopo

- Terminal Threads do Agent Panel, identificadas pelo `TerminalId` durável já existente.
- Terminais locais.
- Detecção de OMP iniciado manualmente pelo usuário em uma Terminal Thread.
- Restauração sob demanda, quando o usuário abre a thread; nunca quando o workspace inteiro carrega.

Fora de escopo: panes de terminal genéricos, SSH/remoto, reanexação de PTYs vivos, perfis OMP personalizados, `PI_CODING_AGENT_DIR`, `omp --no-extensions` e suporte operacional a agentes além de OMP.

## Modelo persistido

`TerminalThreadMetadata` armazena `agent_session: Option<TerminalAgentSession>`.

```rust
pub struct TerminalAgentSession {
    pub agent_id: String,
    pub resume_target: String,
    pub working_directory: PathBuf,
}
```

A tabela `sidebar_terminal_threads` recebe três colunas anuláveis: `agent_id`, `agent_resume_target` e `agent_working_directory`. A migration é aditiva; a ausência dos três valores significa uma Terminal Thread genérica.

O banco nunca armazena um comando de shell. Um registro estático converte `agent_id` e `resume_target` em um comando seguro. Inicialmente a única entrada é `omp`, cujo alvo é o ID de sessão autoritativo publicado pelo próprio OMP.

## Captura automática

Antes de abrir uma Terminal Thread local, Zed fornece no ambiente do shell um `TerminalId`, caminho privado de evento e token aleatório. Um `TerminalAgentSessionReporter` observa somente esses arquivos registrados no diretório de dados do Zed.

Zed instala uma extensão OMP gerenciada e marcada em `~/.omp/agent/extensions/zed-terminal-agent-session.ts`. Ela é descoberta pelo mecanismo nativo de extensões do OMP e não tem efeito fora de um terminal com as variáveis `ZED_*` fornecidas por Zed. Zed nunca sobrescreve um arquivo com o mesmo nome que não contenha seu marcador de propriedade.

A extensão lê `ctx.sessionManager.getSessionId()`, `getSessionFile()` e `ctx.cwd` nos eventos de ciclo de sessão. Só publica uma associação retomável quando o ID e o arquivo de sessão existem; emite uma associação vazia para limpar uma sessão anterior que seja efêmera ou ainda não materializada. Ela grava JSON em um arquivo temporário e renomeia atomicamente para o caminho registrado. O receptor confere versão, tamanho, `TerminalId`, token, agente conhecido, formato do ID OMP e diretório antes de alterar metadata. Um marcador de PID herdado impede filhos/subagentes de substituírem a associação do processo OMP principal.

Uma sessão nova ou um `/resume` posterior substitui o alvo salvo da mesma Terminal Thread. Fechar a Terminal Thread remove metadata e o registro de captura, como hoje.

## Restauração

`AgentPanel::restore_terminal` considera `metadata.agent_session` antes do comando global:

1. Cria o shell no `agent_session.working_directory`.
2. Para OMP validado, envia uma única vez `omp --resume '<session_id>'` pelo handshake de inicialização já existente.
3. Não executa `agent.terminal_init_command` em paralelo.
4. Para metadata sem uma sessão de agente, ou para agente/ID desconhecido, preserva o init command global atual.

Reabrir uma Terminal Thread já viva apenas a ativa; não reinsere comando. Se `omp --resume` falhar, o shell permanece aberto e a associação continua salva para uma nova tentativa.

## Invariantes

- Uma Terminal Thread tem no máximo uma associação de agente ativa.
- A associação vem exclusivamente do ID autoritativo publicado pelo agente; nunca de título, texto do terminal, argv, breadcrumb TTY, mtime ou "sessão mais recente".
- O diretório de retomada é o diretório publicado pelo agente, não o worktree nem o último `cd` do shell.
- Dados persistidos não escolhem executáveis, flags arbitrárias ou código shell.
- Dados inválidos, token errado, threads fechadas, agentes desconhecidos e terminais remotos não alteram metadata.
- Falha de extensão/relato não interrompe OMP; apenas elimina a retomada automática.

## Verificação de aceitação

1. A store persiste, recarrega, substitui e limpa `TerminalAgentSession`.
2. Dois `TerminalId`s com alvos distintos não se cruzam.
3. Restaurar OMP escreve exatamente um `omp --resume '<id>'`, não executa o init global e usa o cwd publicado pela sessão.
4. Uma Terminal Thread sem associação continua no caminho existente.
5. Callback inválido não grava associação.
6. A seleção de uma thread restaurada não inicia threads persistidas não selecionadas.

## Referências

- [Orca — recuperação de sessões](https://www.onorca.dev/docs/model/session-restore)
- [Orca — implementação de retomada por agente](https://github.com/stablyai/orca/blob/main/src/shared/agent-session-resume.ts)
- [Orca — extensão OMP gerenciada](https://github.com/stablyai/orca/blob/main/src/main/pi/titlebar-extension-service.ts)
- [OMP — extensões](https://github.com/can1357/oh-my-pi/blob/main/docs/extensions.md)
- [OMP — descoberta de extensões](https://github.com/can1357/oh-my-pi/blob/main/docs/extension-loading.md)
- [OMP — retomada por ID](https://github.com/can1357/oh-my-pi/blob/main/docs/session-switching-and-recent-listing.md)
