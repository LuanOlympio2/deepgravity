mod agent;
mod client;
mod gui;
mod prompt;
pub mod storage;
mod tools;
mod ui;

use agent::Agent;
use clap::Parser;
use client::DeepSeekClient;
use colored::*;
use std::env;
use std::io::{self, Write};
use ui::print_banner;

#[derive(Parser, Debug)]
#[command(name = "deepgravity")]
#[command(author = "Olympio & DeepGravity Team")]
#[command(version = "0.1.0")]
#[command(about = "Assistente autônomo de codificação e sistema no estilo Antigravity movido a DeepSeek", long_about = None)]
struct Cli {
    /// Prompt inicial ou tarefa direta a ser executada (modo único)
    #[arg(value_name = "PROMPT")]
    prompt: Option<String>,

    /// Iniciar a interface gráfica desktop (estilo web chat nativo ultra leve)
    #[arg(short = 'g', long = "gui")]
    gui: bool,

    /// URL base da API OpenAI-compatível do DeepSeek Web
    #[arg(short, long, default_value = "http://127.0.0.1:8000/v1")]
    endpoint: String,

    /// Modelo a ser utilizado (deepseek-web ou deepseek-web-reasoner)
    #[arg(short, long, default_value = "deepseek-web")]
    model: String,

    /// Autoriza automaticamente todos os comandos de terminal sem confirmação
    #[arg(short = 'y', long = "auto-approve")]
    auto_approve: bool,

    /// Diretório de trabalho para o agente
    #[arg(short, long)]
    dir: Option<String>,
}

fn main() -> anyhow::Result<()> {
    let mut cli = Cli::parse();
    if let Some(arg0) = env::args().next() {
        if arg0.ends_with("deepgravity-gui") {
            cli.gui = true;
        }
    }

    let current_dir = match cli.dir.clone() {
        Some(d) => d,
        None => env::current_dir()?
            .to_string_lossy()
            .to_string(),
    };

    // Modo Interface Gráfica Nativa (GUI)
    if cli.gui {
        return gui::run_gui(cli.endpoint, cli.model, current_dir, cli.auto_approve, cli.prompt.clone())
            .map_err(|e| anyhow::anyhow!("{}", e));
    }

    // Modo Terminal CLI / REPL
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    rt.block_on(run_cli(cli, current_dir))
}

async fn run_cli(cli: Cli, current_dir: String) -> anyhow::Result<()> {
    let client = DeepSeekClient::new(&cli.endpoint, &cli.model);
    let mut agent = Agent::new(client, current_dir.clone(), cli.auto_approve);

    // Se o usuário passou um prompt via argumentos de linha de comando:
    if let Some(initial_prompt) = cli.prompt {
        print_banner(&cli.model, &cli.endpoint, cli.auto_approve);
        println!("{} {}", "⚡ Executando tarefa:".bold().bright_green(), initial_prompt.white());
        let res = agent.process_user_input(&initial_prompt).await?;
        println!("\n{}\n{}", "✨ Resposta do DeepGravity:".bold().bright_cyan(), res);
        return Ok(());
    }

    // Modo REPL Interativo no Terminal
    print_banner(&cli.model, &cli.endpoint, cli.auto_approve);
    println!("{} {}\n", "📁 Diretório ativo:".dimmed(), current_dir.bright_yellow());

    let stdin = io::stdin();
    loop {
        print!("{}", "deepgravity ❯ ".bold().bright_green());
        io::stdout().flush()?;

        let mut line = String::new();
        if stdin.read_line(&mut line)? == 0 {
            // EOF (Ctrl+D)
            println!("\n{}", "Até logo!".dimmed());
            break;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Comandos internos especiais
        if trimmed.starts_with('/') {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            match parts[0] {
                "/exit" | "/quit" | "/q" => {
                    println!("{}", "Encerrando DeepGravity. Até mais!".bright_blue());
                    break;
                }
                "/clear" => {
                    agent.reset();
                    println!("{}", "✔ Histórico da conversa reiniciado.".green());
                    continue;
                }
                "/model" => {
                    if parts.len() > 1 {
                        let new_model = parts[1];
                        agent.client.model = new_model.to_string();
                        println!("{} {}", "✔ Modelo alterado para:".green(), new_model.bright_yellow());
                    } else {
                        println!("{} {}", "Modelo atual:".cyan(), agent.client.model.bright_yellow());
                        println!("{}", "Uso: /model <deepseek-web | deepseek-web-reasoner>".dimmed());
                    }
                    continue;
                }
                "/help" => {
                    println!("\n{}", "--- Ajuda dos Comandos Internos ---".bright_cyan());
                    println!("  {} - Exibe este menu de ajuda", "/help".bold());
                    println!("  {} - Limpa a memória de conversa do agente", "/clear".bold());
                    println!("  {} - Mostra ou altera o modelo atual", "/model [nome]".bold());
                    println!("  {} - Finaliza a sessão do aplicativo\n", "/exit".bold());
                    continue;
                }
                _ => {
                    println!("{} {}", "Comando desconhecido:".red(), parts[0]);
                    println!("{}", "Digite /help para listar os comandos disponíveis.".dimmed());
                    continue;
                }
            }
        }

        // Processa prompt com o agente
        match agent.process_user_input(trimmed).await {
            Ok(output) => {
                println!("\n{}\n{}\n", "✨ Resposta do DeepGravity:".bold().bright_cyan(), output);
            }
            Err(e) => {
                println!("\n{} {}\n", "❌ Erro ao processar requisição:".bold().red(), e);
            }
        }
    }

    Ok(())
}
