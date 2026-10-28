# 🌌 DeepGravity

> **Agente Autônomo de Codificação e Automação de Sistemas em Rust Nativo**  
> Inspirado nas capacidades do **Google Antigravity** e **Claude Code**, com interface gráfica ultra-leve (~85 MB de RAM) e movido pelo motor **DeepSeek** via API universal.

[![Rust](https://img.shields.io/badge/Rust-1.75%2B-orange.svg?style=flat-square&logo=rust)](https://www.rust-lang.org/)
[![GUI](https://img.shields.io/badge/GUI-egui%200.29-blue.svg?style=flat-square)](https://github.com/emilk/egui)
[![RAM](https://img.shields.io/badge/RAM-~85%20MB-brightgreen.svg?style=flat-square)](#-performance--baixo-consumo)
[![License](https://img.shields.io/badge/License-MIT-lightgrey.svg?style=flat-square)](LICENSE)

---

## 🌟 Visão Geral

O **DeepGravity** é um assistente autônomo para engenharia de software e controle de sistema. Diferente de soluções pesadas baseadas em Electron (que consomem centenas de megabytes ou gigabytes de memória), o DeepGravity foi escrito inteiramente em **Rust**, utilizando **Tokio** para operações assíncronas concorrentes e **egui/eframe** para renderização gráfica imediata com aceleração de hardware.

Ele atua como um programador par autônomo: lê a estrutura de projetos, edita código cirurgicamente com diffs em tempo real, roda comandos no terminal, ouve instruções por voz e executa tarefas completas do início ao fim.

---

## ✨ Principais Funcionalidades

- **Interface Visual Moderna (Dark Glass)**: Estilo refinado com a identidade visual da DeepSeek, barra lateral retrátil, navegação por abas e renderização rica de Markdown (tabelas listradas, blocos de código com botão de cópia, citações e cards expansíveis).
- **Consumo de Memória Mínimo (~85 MB)**: Construído em Rust nativo, sem navegadores embutidos, sem WebView e sem Electron.
- **Ciclo ReAct Autônomo**: A IA inspeciona arquivos, executa comandos no terminal, lê os retornos e itera até resolver o problema proposto.
- **Histórico Persistente de Sessões**: Conversas salvas automaticamente em disco (`~/.local/share/deepgravity/sessions/`), permitindo alternar entre chats anteriores restaurando todo o contexto.
- **Gerenciador de Workspaces / Projetos**: Alterne entre diferentes pastas de trabalho com 1 clique; os comandos e edições passam a operar automaticamente no diretório selecionado.
- **Controle de Segurança & Autonomia**:
  - Modal interativo para autorizar comandos de terminal potencialmente destrutivos (`run_command`).
  - Switch **"Auto-aprovar"** em tempo real com sincronização atômica (`Arc<AtomicBool>`).
- **Botão de Parada de Emergência (`Esc`)**: Interrompe o raciocínio da IA e mata instantaneamente qualquer processo bash filho em execução (`kill_on_drop`).
- **Suporte a Mídia & Voz**:
  - 🎙 **Gravação de Áudio com Transcrição Automática**: Gravação nativa com `arecord` (16kHz WAV) e transcrição integrada fala-para-texto.
  - 📎 **Anexo de Fotos e Documentos**: Envio multimodal com miniaturas visuais e codificação base64.
- **Correção de Acentos e Teclas Mortas no Linux**: Vendorização e patch do `egui-winit` para garantir funcionamento perfeito de caracteres acentuados (`ç`, `ã`, `é`, `ô`, `à`) no teclado ABNT2/Linux.
- **Entrada Multilinha Inteligente**: `Shift + Enter` para quebrar linhas e `Enter` para envio instantâneo.

---

## 🏗️ Arquitetura do Sistema

O DeepGravity é dividido em camadas modulares de alta performance:

```
┌─────────────────────────────────────────────────────────────┐
│                    DeepGravity GUI (Rust)                   │
│   eframe / egui 0.29.1 · Tema Dark Glass · ~85 MB de RAM    │
└──────────────────────────────┬──────────────────────────────┘
                               │
            ┌──────────────────┴──────────────────┐
            ▼                                     ▼
┌───────────────────────────┐         ┌───────────────────────┐
│     Agente ReAct Tokio    │         │  Interface de Mídia   │
│  - Loop assíncrono        │         │  - Zenity / Drag&Drop │
│  - Ferramentas nativas    │         │  - arecord (Áudio)    │
│  - Poda de contexto       │         │  - Transcrição STT    │
└─────────────┬─────────────┘         └───────────────────────┘
              │
              ▼ (Chamadas de Ferramentas Nativas)
┌─────────────────────────────────────────────────────────────┐
│ run_command (bash) · read_file · write_file · edit_file     │
└─────────────────────────────┬───────────────────────────────┘
                              │
                              ▼ (HTTP / OpenAI Protocol)
┌─────────────────────────────────────────────────────────────┐
│     Universal API Gateway Bridge (127.0.0.1:8000/v1)        │
│   Powered by Fahremback/deepseek-web-universal-api          │
└─────────────────────────────┬───────────────────────────────┘
                              │ (WebSocket / HTTP + C PoW Solver)
                              ▼
┌─────────────────────────────────────────────────────────────┐
│           DeepSeek (Chat & R1 Reasoner Engine)              │
└─────────────────────────────────────────────────────────────┘
```

---

## 🔗 Dependência: `Fahremback/deepseek-web-universal-api`

O DeepGravity utiliza como motor de inteligência artificial a ponte desenvolvida por **[Fahremback/deepseek-web-universal-api](https://github.com/Fahremback/deepseek-web-universal-api)**.

### O que o projeto do Fahremback faz?
1. **Emulação do Protocolo Web**: O repositório implementa um gateway local compatível com o padrão da OpenAI (`/v1/chat/completions`) que se comunica diretamente com a interface web da DeepSeek.
2. **Solucionador PoW em C (`ds_pow_solver`)**: Resolve os desafios matemáticos de Proof-of-Work exigidos pelos servidores da DeepSeek em frações de segundo.
3. **Cofre de Credenciais de SO (`keyring`)**: Armazena o token de sessão do usuário no cofre nativo do sistema operacional de forma segura, permitindo que as requisições ocorram sem necessidade de manter navegadores abertos.
4. **Suporte Híbrido**: Disponibiliza os modelos `deepseek-web` (DeepSeek Chat rápido) e `deepseek-web-reasoner` (DeepSeek R1 com raciocínio analítico profundo).

### Como iniciar o gateway do Fahremback:

```bash
# Clone o repositório do gateway
git clone https://github.com/Fahremback/deepseek-web-universal-api.git
cd deepseek-web-universal-api

# Crie o ambiente virtual e instale as dependências
python3 -m venv .venv
./.venv/bin/pip install -r requirements.txt

# Compile o solucionador PoW em C (se necessário)
gcc -O3 -shared -fPIC -o ds_pow_solver ds_pow_solver.c

# Inicie o servidor daemon em segundo plano
./.venv/bin/python openai_server.py --host 127.0.0.1 --port 8000
```

---

## 🚀 Compilação e Instalação

### Pré-requisitos
- **Rust e Cargo** (versão 1.75 ou superior):
  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  ```
- **Dependências de Sistema (Linux/Debian/Ubuntu/Mint)**:
  ```bash
  sudo apt install -y build-essential libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libx11-dev libasound2-dev zenity alsa-utils
  ```

### Compilar o DeepGravity
```bash
# Clone este repositório
git clone https://github.com/SEU_USUARIO/deepgravity.git
cd deepgravity

# Compile em modo Release (otimizado)
cargo build --release

# Opcional: Instale o binário no seu PATH
cp target/release/deepgravity ~/.cargo/bin/
```

### Executar
```bash
# Modo Interface Gráfica (Recomendado)
deepgravity --gui

# Modo Terminal / CLI Interativo
deepgravity

# Executar instrução única direta
deepgravity "Resuma a arquitetura deste projeto"
```

---

## ⚙️ Configuração do Token de Acesso

Caso o token da sessão expire ou você queira atualizar sua conta:

1. No navegador, acesse **[chat.deepseek.com](https://chat.deepseek.com)** e faça login.
2. Abra o Console de Desenvolvedor (**F12** > aba **Console**), cole o comando e pressione Enter:
   ```javascript
   JSON.parse(localStorage.getItem('userToken')).value
   ```
3. Copie o token retornado.
4. Abra o DeepGravity, vá até a aba **⚙ Ajustes** na barra lateral, cole o token no campo e clique em **Salvar Token**.
   *(O DeepGravity aceita tanto o código puro quanto o JSON bruto copiado do navegador, fazendo a limpeza e validação automaticamente).*

---

## 🛡️ Prevenção do Erro `Prompt exceeds maximum size`

O DeepGravity conta com um mecanismo automático de **Poda Inteligente de Contexto** (`prune_messages_for_request`):
- Saídas de comandos e arquivos muito extensos são truncadas de forma inteligente (mantendo o cabeçalho e o final do retorno).
- Quando a conversa acumula muitos turnos, mensagens antigas deslizam para fora do buffer mantendo a mensagem de sistema inicial e as interações recentes dentro do limite seguro da API (140k caracteres), evitando quebras por excesso de tamanho de prompt.

---

## 📄 Licença

Distribuído sob a licença MIT. Veja `LICENSE` para mais informações.

Agradecimentos especiais a **[Fahremback](https://github.com/Fahremback)** pelo desenvolvimento do [deepseek-web-universal-api](https://github.com/Fahremback/deepseek-web-universal-api).
