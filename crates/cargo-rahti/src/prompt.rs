//! The questions the scaffold asks.
//!
//! Both take their default rather than hanging when there is nobody to
//! answer. A scaffold that blocks forever in CI is worse than one that
//! documents its default and takes it.

use std::io::{self, IsTerminal, Write};

/// Ask a yes/no question, defaulting to no.
///
/// No rather than yes, because every question here asks whether to *add*
/// something, and the flag that answers it — `--tailwind`, `--db` — only ever
/// says yes. Omitting the flag is how you say no, so the question has to
/// default the same way: otherwise pressing enter would do the opposite of
/// leaving the flag out, and one of the two would always surprise someone.
///
/// A pipe, a CI job or a closed stdin gets the default rather than a hang:
/// `is_terminal` is checked first, and a read that returns nothing — which is
/// what end-of-input looks like — is treated the same way. A scaffold that
/// blocks forever waiting for an answer nobody can give is worse than one
/// that documents its default and takes it.
pub fn confirm(question: &str) -> bool {
    if !io::stdin().is_terminal() {
        return false;
    }

    loop {
        print!("{question} [y/N] ");
        let _ = io::stdout().flush();

        let mut answer = String::new();
        if io::stdin().read_line(&mut answer).unwrap_or(0) == 0 {
            println!();
            return false;
        }

        match answer.trim().to_ascii_lowercase().as_str() {
            "" | "n" | "no" => return false,
            "y" | "yes" => return true,
            _ => println!("Please answer y or n."),
        }
    }
}

/// Ask which of `options` — the first is the default.
///
/// Answered by name or by number, because a list of three is faster to pick
/// from by number and easier to script by name. Same non-interactive rule as
/// [`confirm`]: no terminal, or no input, takes the first option.
pub fn choose(question: &str, options: &[&str]) -> String {
    let default = options.first().copied().unwrap_or_default().to_string();

    if !io::stdin().is_terminal() {
        return default;
    }

    loop {
        println!("{question}");
        for (i, option) in options.iter().enumerate() {
            let note = if i == 0 { " (default)" } else { "" };
            println!("  {}) {option}{note}", i + 1);
        }
        print!("> ");
        let _ = io::stdout().flush();

        let mut answer = String::new();
        if io::stdin().read_line(&mut answer).unwrap_or(0) == 0 {
            println!();
            return default;
        }

        let answer = answer.trim().to_ascii_lowercase();
        if answer.is_empty() {
            return default;
        }
        if let Some(picked) = answer
            .parse::<usize>()
            .ok()
            .and_then(|n| options.get(n.wrapping_sub(1)))
        {
            return (*picked).to_string();
        }
        if let Some(picked) = options.iter().find(|o| o.eq_ignore_ascii_case(&answer)) {
            return (*picked).to_string();
        }

        println!(
            "Please answer with a number or one of: {}.",
            options.join(", ")
        );
    }
}
