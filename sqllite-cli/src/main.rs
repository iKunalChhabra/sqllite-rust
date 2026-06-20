//! sqllite3 command-line shell.

use anyhow::Result;
use clap::Parser as ClapParser;
use sqllite_core::Connection;
use std::io::{self, BufRead, Write};

#[derive(ClapParser)]
#[command(name = "sqllite3", about = "SQLite-compatible shell (pure Rust)")]
struct Args {
    /// Database file path (use :memory: for in-memory database)
    database: Option<String>,

    /// SQL statement to execute
    #[arg(short = 'c')]
    command: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let db_path = args.database.unwrap_or_else(|| ":memory:".into());
    let mut conn = Connection::open(&db_path)?;

    if let Some(sql) = args.command {
        run_sql(&mut conn, &sql)?;
        return Ok(());
    }

    // Interactive mode
    println!("sqllite3 version 0.1.0 (pure Rust)");
    println!("Enter \".help\" for usage hints.");
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    loop {
        print!("sqllite> ");
        stdout.flush()?;
        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == ".quit" || line == ".exit" {
            break;
        }
        if line == ".help" {
            println!(".help     Show this message");
            println!(".quit     Exit the shell");
            println!(".tables   List tables");
            continue;
        }
        if line == ".tables" {
            match conn.execute("SELECT name FROM sqlite_schema WHERE type='table'") {
                Ok(rows) => {
                    for row in rows {
                        println!("{row}");
                    }
                }
                Err(e) => eprintln!("Error: {e}"),
            }
            continue;
        }
        match run_sql(&mut conn, line) {
            Ok(()) => {}
            Err(e) => eprintln!("Error: {e}"),
        }
    }
    Ok(())
}

fn run_sql(conn: &mut Connection, sql: &str) -> Result<()> {
    let rows = conn.execute(sql)?;
    if !rows.is_empty() {
        let mut stdout = io::stdout();
        for (i, val) in rows.iter().enumerate() {
            if i > 0 {
                write!(stdout, " ")?;
            }
            write!(stdout, "{val}")?;
        }
        writeln!(stdout)?;
    }
    Ok(())
}
