use colored::*;
use similar::{ChangeTag, TextDiff};

pub fn print_banner(model: &str, endpoint: &str, auto_approve: bool) {
    println!("{}", "========================================================".bright_blue());
    println!("{}", "               🚀 DEEPGRAVITY AGENT 🚀                  ".bold().bright_cyan());
    println!("{}", "   Assistente Autônomo de Código e Automação de Sistema ".bright_white());
    println!("{}", "========================================================".bright_blue());
    println!("  {} {}", "• Endpoint:".dimmed(), endpoint.bright_yellow());
    println!("  {} {}", "• Modelo:  ".dimmed(), model.bright_green());
    println!(
        "  {} {}",
        "• Autonomia:".dimmed(),
        if auto_approve {
            "Total (--auto-approve)".bright_red().bold()
        } else {
            "Interativa (confirmação prévia de comandos)".bright_green()
        }
    );
    println!("  {} {}\n", "• Comandos:".dimmed(), "/help, /model, /clear, /exit".white());
}

pub fn print_tool_start(tool_name: &str, args_json: &str) {
    println!(
        "\n{} {}{}{}",
        "⚙️  [Chamando Ferramenta]".bold().bright_magenta(),
        tool_name.bright_cyan().bold(),
        " -> ".dimmed(),
        args_json.bright_white()
    );
}

pub fn print_tool_result(tool_name: &str, success: bool, output_summary: &str) {
    let status_badge = if success {
        "✔ SUCESSO".green().bold()
    } else {
        "✖ FALHA".red().bold()
    };

    println!(
        "{} {} {}: {}\n",
        "└─".dimmed(),
        tool_name.dimmed(),
        status_badge,
        output_summary.bright_black()
    );
}

pub fn print_diff(old_text: &str, new_text: &str) {
    let diff = TextDiff::from_lines(old_text, new_text);
    println!("{}", "--- Alterações no arquivo ---".dimmed());
    for change in diff.iter_all_changes() {
        let sign = match change.tag() {
            ChangeTag::Delete => "-".red(),
            ChangeTag::Insert => "+".green(),
            ChangeTag::Equal => " ".normal(),
        };
        print!("{}{}", sign, change);
    }
    println!("{}", "-----------------------------".dimmed());
}
