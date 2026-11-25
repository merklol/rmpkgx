use clap::Parser;
use rmpkgx::{
    cleanup::plan::Plan,
    cli::{Cli, Command},
    config,
    core::{discovery, disk_usage, scanner},
};
use std::{
    io::{self, IsTerminal, Write},
    sync::atomic::AtomicBool,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing::debug!(version = env!("CARGO_PKG_VERSION"), "starting rmpkgx");
    let cli = Cli::parse();
    if cli.command.is_none() {
        return rmpkgx::tui::run().await;
    }
    anyhow::ensure!(
        rmpkgx::platform::macos::supported(),
        "rmpkgx requires macOS"
    );
    let command = cli.command.expect("command checked");
    tokio::task::spawn_blocking(move || run(command)).await?
}
fn run(command: Command) -> anyhow::Result<()> {
    let cancel = AtomicBool::new(false);
    if let Command::Orphans { json } = command {
        let report = rmpkgx::core::orphans::inspect(&cancel)?;
        if json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            println!("{}", report.text());
        }
        return Ok(());
    }
    match &command {
        Command::History { id, json } => {
            let journals = if let Some(id) = id {
                vec![rmpkgx::cleanup::history::read(id)?]
            } else {
                rmpkgx::cleanup::history::list()?
            };
            if *json {
                println!("{}", serde_json::to_string_pretty(&journals)?);
            } else {
                for journal in journals {
                    println!("{}", journal.text());
                    if id.is_some() {
                        println!("{}", serde_json::to_string_pretty(&journal)?);
                    }
                }
            }
            return Ok(());
        }
        Command::Recover { id, json } => {
            let journal = rmpkgx::cleanup::history::read(id)?;
            let app = journal.application()?;
            let mut report = scanner::scan(app, &config::data_roots(), &cancel)?;
            for finding in &mut report.items {
                finding.selected = false;
            }
            if *json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({ "journal": journal, "fresh_scan": report, "read_only": true })
                    )?
                );
            } else {
                println!(
                    "{}\nFresh read-only recovery scan; historical paths do not authorize cleanup.",
                    journal.text()
                );
                for f in report.items {
                    println!(
                        "{:?} {} bytes {} — {}",
                        f.confidence,
                        f.bytes,
                        f.path.display(),
                        f.reason
                    );
                }
                for issue in report.issues {
                    eprintln!("Uninspected: {}: {}", issue.path.display(), issue.message);
                }
                println!(
                    "Nothing selected or moved. Missing app bundles do not prevent this data scan."
                );
            }
            return Ok(());
        }
        _ => {}
    }
    let report = discovery::discover(&config::application_roots(), &cancel)?;
    match command {
        Command::List { json } => {
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                for a in &report.items {
                    println!(
                        "{}\t{}\t{}{}",
                        a.name,
                        a.bundle_id,
                        a.path.display(),
                        if a.protected { " [protected]" } else { "" }
                    );
                }
                eprintln!("{} inspection issues", report.issues.len());
            }
        }
        Command::Scan { application, json } => show(&report, &application, json, false, true)?,
        Command::Remove {
            application,
            dry_run,
        } => show(&report, &application, false, true, dry_run)?,
        Command::Analyze { json } => {
            let mut rows = vec![];
            for app in &report.items {
                let (bytes, issues) = disk_usage::measure(&app.path, &cancel)?;
                rows.push((app, bytes, issues));
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&(rows, report.issues))?);
            } else {
                for (a, bytes, issues) in rows {
                    println!(
                        "{}\t{} bytes\t{} issues\t{}",
                        a.name,
                        bytes,
                        issues.len(),
                        a.path.display()
                    );
                }
                eprintln!("{} discovery issues", report.issues.len());
            }
        }
        Command::Orphans { .. } | Command::History { .. } | Command::Recover { .. } => {
            unreachable!()
        }
    }
    Ok(())
}
fn show(
    report: &rmpkgx::core::Report<rmpkgx::core::application::Application>,
    query: &str,
    json: bool,
    remove: bool,
    dry_run: bool,
) -> anyhow::Result<()> {
    let app = discovery::resolve(&report.items, query)?;
    let cancel = AtomicBool::new(false);
    let scan = scanner::scan(app, &config::data_roots(), &cancel)?;
    let (bundle_bytes, mut issues) = disk_usage::measure(&app.path, &cancel)?;
    issues.extend(report.issues.clone());
    issues.extend(scan.issues);
    let plan = Plan {
        application: app.clone(),
        bundle_bytes,
        findings: scan.items,
        issues,
        data_only: false,
        execution_enabled: cfg!(target_os = "macos"),
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&plan)?);
    } else {
        println!(
            "{} — {}\nBundle: {} bytes\n{}",
            app.name,
            app.bundle_id,
            bundle_bytes,
            app.path.display()
        );
        for f in &plan.findings {
            println!(
                "[{}] {:?} {} bytes {} — {}",
                if f.selected { "x" } else { " " },
                f.confidence,
                f.bytes,
                f.path.display(),
                f.reason
            );
        }
        for issue in &plan.issues {
            eprintln!("Uninspected: {}: {}", issue.path.display(), issue.message);
        }
        println!(
            "Scan report: {} inspection issues; leftover discovery is not exhaustive.",
            plan.issues.len()
        );
    }
    if !json {
        let summary = plan.summary();
        println!("Selected data: {} paths / {} logical bytes; remaining: {}; shared: {}; inspection issues: {}", summary.selected, summary.selected_bytes, summary.remaining, summary.shared, summary.inspection_issues);
        println!("{}", rmpkgx::platform::macos::running_status(&app.path));
    }
    if remove {
        plan.validate(&config::data_roots())?;
        if !dry_run {
            let mut review = rmpkgx::cleanup::review::Review::prepare(&plan)?;
            review.revalidate()?;
            println!(
                "{}",
                review.text(&rmpkgx::platform::macos::running_status(&app.path))
            );
            anyhow::ensure!(
                io::stdin().is_terminal() && io::stdout().is_terminal(),
                "interactive confirmation required; use --dry-run for noninteractive execution"
            );
            while let Some(item) = review.next_consent().cloned() {
                println!("Manual review: {:?} — {}\nEvidence: {}\nOwnership is unverified; other app versions may use this path.", item.confidence, item.path.display(), item.reason);
                print!(
                    "Type this exact absolute path to accept ownership (anything else cancels): "
                );
                io::stdout().flush()?;
                let mut answer = String::new();
                io::stdin().read_line(&mut answer)?;
                let answer = answer.trim_end_matches(['\r', '\n']);
                if std::path::Path::new(answer) != item.path {
                    println!("Cancelled. No files moved.");
                    return Ok(());
                }
                review.approve_path(&item.path)?;
            }
            print!(
                "Type /confirm to move exactly these objects to Trash (anything else cancels): "
            );
            io::stdout().flush()?;
            let mut answer = String::new();
            io::stdin().read_line(&mut answer)?;
            if answer.trim() != "/confirm" {
                println!("Cancelled. No files moved.");
                return Ok(());
            }
            let outcome = review.execute()?;
            println!("{}", outcome.text());
            anyhow::ensure!(
                !outcome.has_failures(),
                "cleanup had failures; see per-path report"
            );
        }
    }
    Ok(())
}
