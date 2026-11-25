mod theme;

use crate::{
    cleanup::plan::Plan,
    config,
    core::{application::Application, discovery, disk_usage, scanner},
};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    widgets::{Block, Paragraph, Wrap},
};
use std::{
    io::{self, IsTerminal},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}
struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}
enum Message {
    Orphans(anyhow::Result<crate::core::orphans::OrphanReport>),
    CatalogRefreshed {
        revision: u64,
        result: anyhow::Result<crate::core::Report<Application>>,
    },
    SettingsOpened(anyhow::Result<()>),
    Restore(anyhow::Result<crate::cleanup::restore::RestorePlan>),
    Restored(anyhow::Result<String>),
    History(anyhow::Result<Vec<crate::cleanup::history::Journal>>),
    Apps(anyhow::Result<crate::core::Report<Application>>),
    Plan(anyhow::Result<(Plan, String)>),
    Review(anyhow::Result<(crate::cleanup::review::Review, String)>),
    Cleaned(anyhow::Result<crate::cleanup::trash::Outcome>),
}
fn launch_scan(
    app: Application,
    cancel: Arc<AtomicBool>,
    tx: tokio::sync::mpsc::UnboundedSender<Message>,
    progress_tx: tokio::sync::watch::Sender<(u64, std::path::PathBuf)>,
) {
    tokio::task::spawn_blocking(move || {
        let result = (|| {
            let mut checked = 0u64;
            let mut last_update = Instant::now();
            let mut progress = |path: &std::path::Path| {
                checked = checked.saturating_add(1);
                if checked == 1 || last_update.elapsed() >= Duration::from_millis(80) {
                    progress_tx.send_replace((checked, path.to_owned()));
                    last_update = Instant::now();
                }
            };
            let scan =
                scanner::scan_with_progress(&app, &config::data_roots(), &cancel, &mut progress)?;
            let (bundle_bytes, mut issues) =
                disk_usage::measure_with_progress(&app.path, &cancel, &mut progress)?;
            let running = crate::platform::macos::running_status(&app.path);
            issues.extend(scan.issues);
            Ok((
                Plan {
                    application: app,
                    bundle_bytes,
                    findings: scan.items,
                    issues,
                    data_only: false,
                    execution_enabled: cfg!(target_os = "macos"),
                },
                running,
            ))
        })();
        let _ = tx.send(Message::Plan(result));
    });
}
pub async fn run() -> anyhow::Result<()> {
    anyhow::ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "interactive terminal required; use list --json or scan NAME --json"
    );
    anyhow::ensure!(crate::platform::macos::supported(), "rmpkgx requires macOS");
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let (progress_tx, mut progress_rx) =
        tokio::sync::watch::channel((0u64, std::path::PathBuf::new()));
    let mut running_status = String::new();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    let worker_tx = tx.clone();
    tokio::task::spawn_blocking(move || {
        let _ = worker_tx.send(Message::Apps(
            discovery::discover(&config::application_roots(), &worker_cancel).map_err(Into::into),
        ));
    });
    let mut apps: Vec<Application> = vec![];
    let catalog_cancel = Arc::new(AtomicBool::new(false));
    let _catalog_shutdown = CancelOnDrop(catalog_cancel.clone());
    let mut catalog_revision = 0u64;
    let mut catalog_refresh_pending = false;
    let mut catalog_refresh_running = false;
    let mut catalog_notice: Option<String> = None;
    let mut history: Option<Vec<crate::cleanup::history::Journal>> = None;
    let mut history_record: Option<String> = None;
    let mut restore: Option<crate::cleanup::restore::RestorePlan> = None;
    let mut restore_confirm = false;
    let mut plan: Option<Plan> = None;
    let mut status = "Discovering applications…".to_owned();
    let mut busy = true;
    let mut moving = false;
    let mut confirm_move = false;
    let mut cleanup_choice = false;
    let mut cleanup_menu_focused = false;
    let mut search = String::new();
    let mut input = String::new();
    let mut cursor = 0usize;
    let mut scroll = 0usize;
    let mut show_issues = false;
    let mut review_text: Option<String> = None;
    let mut pending_review: Option<crate::cleanup::review::Review> = None;
    loop {
        while let Ok(message) = rx.try_recv() {
            // A catalog refresh is independent of the active screen and worker.
            // It must never clear busy/moving or replace the operation report.
            if let Message::CatalogRefreshed { revision, result } = message {
                catalog_refresh_running = false;
                if revision != catalog_revision {
                    catalog_refresh_pending = true;
                    continue;
                }
                match result {
                    Ok(report) => {
                        apps = report
                            .items
                            .into_iter()
                            .filter(|app| !app.protected)
                            .collect();
                        catalog_notice = (!report.issues.is_empty()).then(|| {
                            format!(
                                "App list refreshed with {} discovery issues",
                                report.issues.len()
                            )
                        });
                    }
                    Err(error) => {
                        catalog_notice = Some(format!("App list refresh failed: {error:#}"))
                    }
                }
                continue;
            }
            busy = false;
            moving = false;
            match message {
                Message::CatalogRefreshed { .. } => {
                    unreachable!("catalog event handled separately")
                }
                Message::SettingsOpened(result) => {
                    status = match result { Ok(()) => "Settings opened. Privacy & Security → Full Disk Access. F5 rechecks; no automatic restore.".into(), Err(e) => format!("{e:#}") };
                }

                Message::Restore(Ok(prepared)) => {
                    restore = Some(prepared);
                    restore_confirm = false;
                    history_record = None;
                    review_text = None;
                    plan = None;
                    cursor = 0;
                    scroll = 0;
                    status = "Select restorable objects with Space, then R to review. Blocked rows explain why.".into();
                }
                Message::Restored(Ok(report)) => {
                    catalog_revision = catalog_revision.wrapping_add(1);
                    catalog_refresh_pending = true;
                    restore = None;
                    restore_confirm = false;
                    review_text = Some(report);
                    cursor = 0;
                    scroll = 0;
                    status = "Restore results · Enter/Esc returns to history".into();
                }
                Message::Restored(Err(e)) => {
                    restore = None;
                    restore_confirm = false;
                    review_text = Some(format!("RESTORE BLOCKED before moving objects\n{e:#}"));
                    status = "Restore preflight failed · Enter/Esc returns to history".into();
                }
                Message::Restore(Err(e)) => {
                    restore = None;
                    restore_confirm = false;
                    status = format!("{e:#}");
                }

                Message::Orphans(result) => {
                    review_text = Some(match result {
                        Ok(report) => report.text(),
                        Err(e) => format!("Leftover inspection failed: {e:#}"),
                    });
                    status = "Possible leftovers · read-only · Enter/Esc returns".into();
                    cursor = 0;
                    scroll = 0;
                }
                Message::History(Ok(entries)) => {
                    status = format!(
                        "{} cleanup journals · Enter inspect · F2 applications",
                        entries.len()
                    );
                    history = Some(entries);
                    cursor = 0;
                    scroll = 0;
                }
                Message::History(Err(e)) => status = format!("History unavailable: {e:#}"),
                Message::Apps(Ok(report)) => {
                    apps = report
                        .items
                        .into_iter()
                        .filter(|app| !app.protected)
                        .collect();
                    status = format!(
                        "{} applications · {} discovery issues",
                        apps.len(),
                        report.issues.len()
                    );
                }
                Message::Plan(Ok((p, running))) => {
                    cleanup_choice = false;
                    running_status = running;
                    status = format!(
                        "{} inspection issues · choose files, then review cleanup below",
                        p.issues.len()
                    );
                    plan = Some(p);
                    cursor = 0;
                    scroll = 0;
                }
                Message::Review(Ok((review, text))) => {
                    status = if review.next_consent().is_some() {
                        "Review the uncertain path shown above · Y accepts this path · N keeps it · Esc cancels"
                    } else {
                        "Review the items and any inspection problems · Y/N chooses · Enter confirms"
                    }
                    .into();
                    confirm_move = false;
                    pending_review = Some(review);
                    review_text = Some(text);
                    cursor = 0;
                    scroll = 0;
                }
                Message::Cleaned(Ok(outcome)) => {
                    catalog_revision = catalog_revision.wrapping_add(1);
                    // Invalidate a concurrently running snapshot after cleanup.
                    catalog_refresh_pending |= catalog_refresh_running;
                    for item in outcome.items.iter().filter(|i| {
                        matches!(
                            i.status,
                            crate::cleanup::trash::MoveStatus::Moved
                                | crate::cleanup::trash::MoveStatus::MovedUnverified
                        )
                    }) {
                        apps.retain(|app| app.path != item.source);
                    }
                    status = if outcome.has_failures() { "Cleanup had failures; review every result. Enter or Esc returns" } else { "Approved objects moved; leftover discovery is not exhaustive. Enter or Esc returns" }.into();
                    review_text = Some(outcome.text());
                    plan = None;
                    cursor = 0;
                    scroll = 0;
                }
                Message::Apps(Err(e))
                | Message::Plan(Err(e))
                | Message::Review(Err(e))
                | Message::Cleaned(Err(e)) => status = e.to_string(),
            }
        }
        if catalog_refresh_pending && !catalog_refresh_running && !busy {
            catalog_refresh_pending = false;
            catalog_refresh_running = true;
            let revision = catalog_revision;
            let sender = tx.clone();
            let cancellation = catalog_cancel.clone();
            tokio::task::spawn_blocking(move || {
                let result = discovery::discover(&config::application_roots(), &cancellation)
                    .map_err(Into::into);
                let _ = sender.send(Message::CatalogRefreshed { revision, result });
            });
        }
        if busy
            && !moving
            && plan.is_none()
            && review_text.is_none()
            && progress_rx.has_changed().unwrap_or(false)
        {
            let (checked, path) = progress_rx.borrow_and_update().clone();
            status = format!(
                "Scanning · {checked} entries visited · {} · Esc cancels",
                path.display()
            );
        }
        let filtered: Vec<_> = apps
            .iter()
            .filter(|a| {
                a.name.to_lowercase().contains(&search.to_lowercase())
                    || a.bundle_id.to_lowercase().contains(&search.to_lowercase())
            })
            .collect();
        let count = review_text.as_ref().map_or_else(
            || {
                plan.as_ref().map_or_else(
                    || {
                        history
                            .as_ref()
                            .map_or(filtered.len(), |entries| entries.len())
                    },
                    |p| {
                        if show_issues {
                            p.issues.len()
                        } else {
                            p.findings.len()
                        }
                    },
                )
            },
            |text| {
                text.lines().count()
                    + if pending_review
                        .as_ref()
                        .is_some_and(|r| r.next_consent().is_some())
                    {
                        2
                    } else {
                        0
                    }
            },
        );
        let count = if let Some(restore) = restore.as_ref().filter(|_| !restore_confirm) {
            restore.entries.len()
        } else {
            count
        };
        cursor = cursor.min(count.saturating_sub(1));
        let current = plan.as_ref().and_then(|p| p.findings.get(cursor));
        if plan.is_none() || review_text.is_some() || busy || show_issues {
            cleanup_menu_focused = false;
        }
        let context = CommandContext {
            cleanup_menu_focused,
            restore_access_denied: restore
                .as_ref()
                .filter(|_| !restore_confirm)
                .and_then(|r| r.entries.get(cursor))
                .is_some_and(|e| e.access_denied),
            restore_list: restore.is_some() && !restore_confirm,
            restore_confirmation: restore_confirm,
            history_retry: history_record
                .as_ref()
                .and_then(|id| history.as_ref()?.iter().find(|j| j.id == *id))
                .is_some_and(|j| {
                    crate::cleanup::recovery::candidates(j).is_ok_and(|paths| !paths.is_empty())
                }),
            recovery_empty: plan
                .as_ref()
                .is_some_and(|p| p.data_only && !p.findings.iter().any(|f| f.selected)),
            history_origin: history.is_some(),
            history_list: history.is_some() && plan.is_none() && review_text.is_none(),
            history_detail: history_record.is_some(),
            busy,
            moving,
            has_plan: plan.is_some(),
            review: review_text.is_some(),
            has_pending_review: pending_review.is_some(),
            needs_approval: pending_review
                .as_ref()
                .is_some_and(|r| r.next_consent().is_some()),
            show_issues,
            has_issues: plan.as_ref().is_some_and(|p| !p.issues.is_empty()),
            can_select: current.is_some_and(|f| !f.shared && !f.review_only && !f.selected)
                && plan.as_ref().is_some_and(|p| !p.application.protected),
            can_unselect: current.is_some_and(|f| f.selected),
            has_apps: history
                .as_ref()
                .map_or(!filtered.is_empty(), |entries| !entries.is_empty()),
        };
        terminal.draw(|frame| {
            frame.render_widget(Block::default().style(Style::default().fg(theme::TEXT).bg(theme::BACKGROUND)), frame.area());
            let areas = Layout::vertical([
                Constraint::Length(3),
                Constraint::Min(1),
                Constraint::Length(if plan.is_some() && review_text.is_none() { 10 } else { 0 }),
                Constraint::Length(if context.available(Command::Remove) && input.is_empty() { 6 } else { 5 }),
            ]).split(frame.area());
            frame.render_widget(Paragraph::new(vec![
                Line::from(vec![Span::styled("rmpkgx", Style::default().fg(theme::TEXT).bold()), Span::styled(format!("  v{}", env!("CARGO_PKG_VERSION")), Style::default().fg(theme::MUTED))]),
                Line::from(Span::styled("Remove apps. Leave no traces.", Style::default().fg(theme::MUTED))),
            ]), areas[0]);
            let height = areas[1].height.saturating_sub(if context.restore_access_denied { 14 } else if restore.is_some() && !restore_confirm { 4 } else { 1 }) as usize;
            if review_text.is_some() {
                cursor = cursor.min(count.saturating_sub(height));
                scroll = cursor;
            } else {
                if cursor < scroll { scroll = cursor; }
                if cursor >= scroll + height.max(1) { scroll = cursor.saturating_sub(height.saturating_sub(1)); }
            }
            let mut lines: Vec<Line<'_>> = if let Some(text) = &review_text {
                ownership_document(text, context.needs_approval, confirm_move).into_iter().skip(scroll).take(height).collect()
            } else if let Some(restore) = &restore {
                restore.entries.iter().enumerate().skip(scroll).take(height).map(|(i, entry)| row(i == cursor, vec![
                    Span::styled(format!("[{}] ", if entry.blocked.is_some() { "!" } else if entry.selected { "x" } else { " " }), Style::default().fg(if entry.blocked.is_some() { theme::RED } else { theme::GREEN })),
                    Span::raw(format!("{} -> {}", entry.source.display(), entry.destination.display())),
                ])).collect()
            } else if let Some(p) = &plan {
                if show_issues {
                    p.issues.iter().enumerate().skip(scroll).take(height).map(|(i, issue)| {
                        row(i == cursor, vec![Span::styled("! ", Style::default().fg(theme::RED)), Span::raw(issue.path.display().to_string())])
                    }).collect()
                } else {
                    p.findings.iter().enumerate().skip(scroll).take(height).map(|(i, f)| {
                        row(i == cursor && !cleanup_menu_focused, vec![
                            Span::styled(if f.selected { "[x] " } else { "[ ] " }, Style::default().fg(if f.selected { theme::GREEN } else { theme::MUTED })),
                            Span::styled(format!("{:>10}  ", human_size(f.bytes)), Style::default().fg(theme::BLUE)),
                            Span::styled(format!("{:?}  ", f.confidence), Style::default().fg(match f.confidence { crate::core::association::Confidence::High => theme::GREEN, crate::core::association::Confidence::Medium => theme::AMBER, crate::core::association::Confidence::Low => theme::RED })),
                            Span::raw(f.path.display().to_string()),
                            Span::styled(if f.review_only { " [inspect only]" } else if f.shared { " [shared]" } else { "" }, Style::default().fg(theme::AMBER)),
                        ])
                    }).collect()
                }
            } else if let Some(entries) = &history {
                entries.iter().enumerate().skip(scroll).take(height).map(|(i, journal)| row(i == cursor, vec![Span::raw(journal.text())])).collect()
            } else {
                filtered.iter().enumerate().skip(scroll).take(height).map(|(i, a)| {
                    row(i == cursor, vec![
                        Span::styled(a.name.clone(), Style::default().fg(theme::TEXT).bold()),
                        Span::styled(format!("  {}", a.version.as_deref().unwrap_or("?")), Style::default().fg(theme::BLUE)),
                        Span::styled(format!("  {}", a.path.display()), Style::default().fg(theme::MUTED)),
                    ])
                }).collect()
            };
            if count == 0 {
                lines.push(Line::from(if restore.is_some() { "No verified moved objects recorded in this journal." } else if plan.is_some() {
                    if show_issues { "No inspection errors reported." } else { "No associated paths found. Review inspection errors before drawing conclusions." }
                } else if busy { "Discovering applications…" } else if history.is_some() { "No cleanup history yet." } else { "No applications match your search." }));
            }
            let title = if restore_confirm { "Confirm restore — no overwrite".into() } else if restore.is_some() { "Restore from Trash · select objects with Space".into() } else if history_record.is_some() { if context.history_retry { "Cleanup journal · Enter re-scan known failed/skipped data" } else { "Cleanup journal · Esc returns to history" }.into() } else if context.history_list { "Cleanup history".into() } else if review_text.is_some() { if context.needs_approval { "Decide whether to include this folder" } else if pending_review.is_some() { "Move these items to Trash?" } else if review_text.as_ref().is_some_and(|t| t.starts_with("Possible leftovers") || t.starts_with("Leftover inspection failed")) { "Possible leftovers · read-only" } else { "Cleanup results" }.into() } else { plan.as_ref().map_or_else(|| "Installed applications".to_string(), |p| {
                format!("{}{} · {} ({})", p.application.name, if p.data_only { " — remaining data only" } else { "" }, if show_issues { "Inspection errors" } else { "Associated paths" }, count)
            }) };
            frame.render_widget(Paragraph::new(lines).block(theme::section(title)), areas[1]);
            if let Some(entry) = restore.as_ref().filter(|_| !restore_confirm).and_then(|r| r.entries.get(cursor)) {
                if entry.access_denied {
                    let panel = Rect { y: areas[1].bottom().saturating_sub(areas[1].height.min(13)), height: areas[1].height.min(13), ..areas[1] };
                    frame.render_widget(Paragraph::new(permission_help())
                        .block(Block::default().borders(ratatui::widgets::Borders::TOP).border_style(Style::default().fg(theme::AMBER)).title(Span::styled(" Restore access is blocked ", Style::default().fg(theme::AMBER).bold())))
                        .wrap(Wrap { trim: true }), panel);
                } else {
                    frame.render_widget(Paragraph::new(entry.blocked.as_deref().unwrap_or("Ready to select. R opens a separate confirmation; nothing moves yet.")).wrap(Wrap { trim: false }).style(Style::default().fg(if entry.blocked.is_some() { theme::AMBER } else { theme::MUTED })), Rect { y: areas[1].bottom().saturating_sub(3), height: areas[1].height.min(3), ..areas[1] });
                }
            }
            if let Some(p) = plan.as_ref().filter(|_| review_text.is_none()) {
                let summary = p.summary();
                let bundle_caption = if p.data_only { "Application bundle excluded — remaining data only".into() } else { format!("Bundle: {} (separate from data selection)", human_size(p.bundle_bytes)) };
                let details = if show_issues {
                    p.issues.get(cursor).map(|issue| format!("Path: {}\nError: {}", issue.path.display(), issue.message)).unwrap_or_default()
                } else {
                    p.findings.get(cursor).map(|f| format!("Path: {}\nEvidence: {}\nConfidence: {:?} · {} · Shared: {}", f.path.display(), f.reason, f.confidence, f.safety, if f.shared { "yes" } else { "no" })).unwrap_or_default()
                };
                frame.render_widget(Paragraph::new(format!("{} · Selected: {} paths / {}\nRemaining: {} paths · Shared: {} · Inspection issues: {}\n{}\nReview required · Sizes may be incomplete; undiscovered leftovers may remain.\n{}", bundle_caption, summary.selected, human_size(summary.selected_bytes), summary.remaining, summary.shared, summary.inspection_issues, running_status, details))
                    .wrap(Wrap { trim: false }).block(theme::section("Inspection details".into())), areas[2]);
            }
            let footer = areas[3];
            let status_line = catalog_notice.as_ref().map_or_else(|| status.clone(), |notice| format!("{status} · {notice}"));
            frame.render_widget(Paragraph::new(status_line).style(Style::default().fg(theme::MUTED)), Rect { height: footer.height.min(1), ..footer });
            let controls_area = Rect { y: footer.y.saturating_add(1), height: footer.height.saturating_sub(1).min(2), ..footer };
            let controls = if input.starts_with('/') {
                Line::from(Span::styled("Enter run · Esc clear input", Style::default().fg(theme::ACCENT).bold()))
            } else { control_hints(&context, input.is_empty()) };
            frame.render_widget(Paragraph::new(controls).wrap(Wrap { trim: true }), controls_area);
            let prompt_area = Rect { y: footer.y.saturating_add(3), height: footer.height.saturating_sub(3).min(if context.available(Command::Remove) && input.is_empty() { 2 } else { 1 }), ..footer };
            let placeholder = if plan.is_some() || review_text.is_some() { "Use the keys shown above." } else { "Search applications…" };
            let prompt = if input.is_empty() && restore_confirm {
                vec![decision_prompt("Restore selected objects?", confirm_move)]
            } else if input.is_empty() && context.available(Command::Remove) {
                cleanup_menu(cleanup_choice, plan.as_ref().is_some_and(|p| p.findings.iter().any(|f| f.selected)), cleanup_menu_focused)
            } else if input.is_empty() && context.recovery_empty && !context.review {
                vec![Line::from("Select remaining data with Space to enable cleanup review.")]
            } else if input.is_empty() && context.available(Command::Confirm) {
                vec![decision_prompt("Move selected objects to Trash?", confirm_move)]
            } else if input.is_empty() && context.review {
                vec![Line::from(if context.needs_approval { "Nothing moves until the final cleanup confirmation." } else { "Enter or Esc returns to applications." })]
            } else {
                vec![Line::from(if input.is_empty() { format!("› {placeholder}") } else { format!("› {input}▏") })]
            };
            frame.render_widget(Paragraph::new(prompt).style(Style::default().fg(if input.is_empty() { theme::MUTED } else { theme::TEXT })), prompt_area);
        })?;
        if event::poll(Duration::from_millis(75))? {
            if let Event::Key(mut key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    if moving {
                        status =
                            "Cleanup is finishing; wait for the per-path report before exiting."
                                .into();
                        continue;
                    }
                    cancel.store(true, Ordering::Relaxed);
                    break;
                }
                if !busy && input.is_empty() && restore.is_some() {
                    match key.code {
                        KeyCode::F(1) => {
                            status = keyboard_help(&context);
                        }
                        KeyCode::Char('s' | 'S')
                            if context.restore_access_denied && !restore_confirm =>
                        {
                            let sender = tx.clone();
                            busy = true;
                            status = "Opening System Settings; no permissions are changed automatically…".into();
                            tokio::task::spawn_blocking(move || {
                                let _ = sender.send(Message::SettingsOpened(
                                    crate::platform::macos::open_system_settings(),
                                ));
                            });
                        }
                        KeyCode::F(5) if !restore_confirm => {
                            let id = restore
                                .as_ref()
                                .expect("restore checked")
                                .journal_id
                                .clone();
                            let sender = tx.clone();
                            busy = true;
                            status =
                                "Rechecking restore paths; previous selections will be cleared…"
                                    .into();
                            tokio::task::spawn_blocking(move || {
                                let result = crate::cleanup::history::read(&id).and_then(|j| {
                                    crate::cleanup::restore::RestorePlan::prepare(&j)
                                });
                                let _ = sender.send(Message::Restore(result));
                            });
                        }
                        KeyCode::Esc if restore_confirm => {
                            restore_confirm = false;
                            review_text = None;
                            cursor = 0;
                            scroll = 0;
                        }
                        KeyCode::Esc => {
                            restore = None;
                            cursor = 0;
                            scroll = 0;
                        }
                        KeyCode::Left | KeyCode::Char('n' | 'N') if restore_confirm => {
                            confirm_move = false
                        }
                        KeyCode::Right | KeyCode::Char('y' | 'Y') if restore_confirm => {
                            confirm_move = true
                        }
                        KeyCode::Char(' ') if !restore_confirm => {
                            if let Some(r) = &mut restore {
                                r.toggle(cursor);
                            }
                        }
                        KeyCode::Enter if restore_confirm && !confirm_move => {
                            restore_confirm = false;
                            review_text = None;
                            cursor = 0;
                            scroll = 0;
                        }
                        KeyCode::Enter if restore_confirm => {
                            let prepared = restore.take().expect("restore checked");
                            let sender = tx.clone();
                            busy = true;
                            moving = true;
                            status = "Restoring selected objects; wait for results…".into();
                            tokio::task::spawn_blocking(move || {
                                let _ = sender.send(Message::Restored(prepared.execute()));
                            });
                        }
                        KeyCode::Char('r' | 'R') if !restore_confirm => {
                            if let Some(r) = &restore {
                                match r.preview() {
                                    Ok(text) => {
                                        review_text = Some(text);
                                        restore_confirm = true;
                                        confirm_move = false;
                                        cursor = 0;
                                        scroll = 0;
                                    }
                                    Err(e) => status = e.to_string(),
                                }
                            }
                        }
                        KeyCode::Up => cursor = cursor.saturating_sub(1),
                        KeyCode::Down => cursor = (cursor + 1).min(count.saturating_sub(1)),
                        _ => {}
                    }
                    continue;
                }
                if key.code == KeyCode::F(3) && !busy && input.is_empty() {
                    if let Some(id) = history_record.clone() {
                        let sender = tx.clone();
                        busy = true;
                        status = "Checking recorded Trash objects for restoration…".into();
                        tokio::task::spawn_blocking(move || {
                            let result = crate::cleanup::history::read(&id)
                                .and_then(|j| crate::cleanup::restore::RestorePlan::prepare(&j));
                            let _ = sender.send(Message::Restore(result));
                        });
                        continue;
                    }
                }
                if key.code == KeyCode::F(4)
                    && !busy
                    && restore.is_none()
                    && plan.is_none()
                    && review_text.is_none()
                    && history.is_none()
                {
                    let sender = tx.clone();
                    let worker_cancel = cancel.clone();
                    let progress_sender = progress_tx.clone();
                    progress_sender.send_replace((0, std::path::PathBuf::new()));
                    cancel.store(false, Ordering::Relaxed);
                    busy = true;
                    input.clear();
                    status = "Inspecting possible leftovers… Esc cancels".into();
                    tokio::task::spawn_blocking(move || {
                        let mut visited = 0u64;
                        let mut updated = Instant::now();
                        let _ = sender.send(Message::Orphans(
                            crate::core::orphans::inspect_with_progress(
                                &worker_cancel,
                                &mut |path| {
                                    visited += 1;
                                    if visited == 1
                                        || updated.elapsed() >= Duration::from_millis(80)
                                    {
                                        progress_sender.send_replace((visited, path.to_owned()));
                                        updated = Instant::now();
                                    }
                                },
                            )
                            .map_err(Into::into),
                        ));
                    });
                    continue;
                }
                if key.code == KeyCode::F(2)
                    && !busy
                    && restore.is_none()
                    && plan.is_none()
                    && review_text.is_none()
                {
                    if history.is_some() {
                        history = None;
                        history_record = None;
                        cursor = 0;
                        scroll = 0;
                        input.clone_from(&search);
                        status = "Installed applications".into();
                    } else {
                        let sender = tx.clone();
                        busy = true;
                        input.clear();
                        status = "Loading cleanup history…".into();
                        tokio::task::spawn_blocking(move || {
                            let _ = sender.send(Message::History(crate::cleanup::history::list()));
                        });
                    }
                    continue;
                }
                if key.code == KeyCode::Esc
                    && input.is_empty()
                    && !busy
                    && history.is_some()
                    && plan.is_none()
                    && review_text.is_none()
                {
                    history = None;
                    cursor = 0;
                    scroll = 0;
                    continue;
                }
                if key.code == KeyCode::Enter && input.is_empty() && !busy {
                    if let Some(id) = if context.history_retry {
                        history_record.take()
                    } else {
                        None
                    } {
                        let sender = tx.clone();
                        let progress_sender = progress_tx.clone();
                        progress_sender.send_replace((0, std::path::PathBuf::new()));
                        let cancellation = cancel.clone();
                        cancel.store(false, Ordering::Relaxed);
                        busy = true;
                        review_text = None;
                        status =
                            "Scanning known remaining data; nothing is selected automatically…"
                                .into();
                        tokio::task::spawn_blocking(move || {
                            let result = (|| {
                                let journal = crate::cleanup::history::read(&id)?;
                                let mut visited = 0u64;
                                let mut updated = Instant::now();
                                let plan = crate::cleanup::recovery::scan(
                                    &journal,
                                    &cancellation,
                                    &mut |path| {
                                        visited = visited.saturating_add(1);
                                        if visited == 1
                                            || updated.elapsed() >= Duration::from_millis(80)
                                        {
                                            progress_sender
                                                .send_replace((visited, path.to_owned()));
                                            updated = Instant::now();
                                        }
                                    },
                                )?;
                                let running =
                                    crate::platform::macos::running_status(&plan.application.path);
                                Ok((plan, running))
                            })();
                            let _ = sender.send(Message::Plan(result));
                        });
                        continue;
                    }
                }
                if key.code == KeyCode::F(1) {
                    status = keyboard_help(&context);
                    continue;
                }
                if input.is_empty()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                {
                    if context.available(Command::Remove)
                        && cleanup_menu_key(
                            key.code,
                            &mut cleanup_menu_focused,
                            &mut cleanup_choice,
                        )
                    {
                        continue;
                    }
                    if context.available(Command::Confirm) || context.needs_approval {
                        if let Some(answer) = dialogue_answer(key.code) {
                            confirm_move = answer;
                            continue;
                        }
                    }
                    if let Some(command) = navigation_key(
                        key.code,
                        &context,
                        if context.available(Command::Remove) {
                            cleanup_choice
                        } else {
                            confirm_move
                        },
                    ) {
                        input = command.into();
                        key.code = KeyCode::Enter;
                    }
                }
                match key.code {
                    KeyCode::Down => cursor = (cursor + 1).min(count.saturating_sub(1)),
                    KeyCode::Up => cursor = cursor.saturating_sub(1),
                    KeyCode::Esc if history_record.is_some() && input.is_empty() && !busy => {
                        history_record = None;
                        review_text = None;
                        cursor = 0;
                        scroll = 0;
                    }
                    KeyCode::Esc if review_text.is_some() && input.is_empty() => {
                        review_text = None;
                        pending_review = None;
                        cursor = 0;
                        scroll = 0;
                    }
                    KeyCode::Esc => {
                        if !input.is_empty() {
                            input.clear();
                            if plan.is_none() {
                                search.clear();
                            }
                        } else if !busy {
                            plan = None;
                            show_issues = false;
                            input.clone_from(&search);
                        }
                        cursor = 0;
                        scroll = 0;
                    }
                    KeyCode::Enter => {
                        let action = if input.starts_with('/') {
                            parse_command(&input)
                        } else if plan.is_none() {
                            Some(Command::Open)
                        } else {
                            None
                        };
                        let action = action.filter(|command| context.available(*command));
                        match action {
                            Some(Command::Quit) if moving => {
                                status = "Cleanup is finishing; wait for the per-path report before exiting.".into();
                                input.clear();
                            }
                            Some(Command::Quit) => {
                                cancel.store(true, Ordering::Relaxed);
                                break;
                            }
                            Some(Command::Cancel) if moving => {
                                status = "An approved native Trash call cannot be interrupted safely. Wait for the report.".into();
                                input.clear();
                            }
                            Some(Command::Cancel) => {
                                if busy {
                                    cancel.store(true, Ordering::Relaxed);
                                    status = "Cancellation requested…".into();
                                } else {
                                    status = "No scan is running.".into();
                                }
                                input.clear();
                            }
                            Some(Command::Back) if !busy => {
                                history_record = None;
                                review_text = None;
                                pending_review = None;
                                plan = None;
                                show_issues = false;
                                if history.is_some() {
                                    input.clear();
                                } else {
                                    input.clone_from(&search);
                                }
                                cursor = 0;
                                scroll = 0;
                            }
                            Some(Command::Errors | Command::Files) if plan.is_some() => {
                                review_text = None;
                                pending_review = None;
                                cleanup_choice = false;
                                show_issues = action == Some(Command::Errors);
                                status = if show_issues {
                                    "Inspection errors"
                                } else {
                                    "Associated files"
                                }
                                .into();
                                input.clear();
                                cursor = 0;
                                scroll = 0;
                            }
                            Some(Command::Open) if plan.is_none() && !busy => {
                                if let Some(entries) = &history {
                                    if let Some(journal) = entries.get(cursor) {
                                        review_text = Some(journal.details());
                                        history_record = Some(journal.id.clone());
                                        cursor = 0;
                                        scroll = 0;
                                        input.clear();
                                        status =
                                            "Journal details · Enter re-scan · Esc back to history"
                                                .into();
                                    }
                                    continue;
                                }
                                if let Some(app) = filtered.get(cursor) {
                                    review_text = None;
                                    pending_review = None;
                                    cancel.store(false, Ordering::Relaxed);
                                    progress_tx.send_replace((0, std::path::PathBuf::new()));
                                    launch_scan(
                                        (*app).clone(),
                                        cancel.clone(),
                                        tx.clone(),
                                        progress_tx.clone(),
                                    );
                                    busy = true;
                                    input.clear();
                                    status = "Scanning… Esc cancels.".into();
                                } else {
                                    status = "No application selected. Type a search query.".into();
                                }
                            }
                            Some(Command::Select | Command::Unselect)
                                if plan.is_some() && !show_issues && review_text.is_none() =>
                            {
                                if let Some(p) = &mut plan {
                                    status = match p.select(cursor, action == Some(Command::Select)) {
                                        Ok(()) => "Selection updated. Review evidence before any future cleanup.".into(),
                                        Err(e) => e.to_string(),
                                    };
                                }
                                input.clear();
                            }
                            Some(Command::Approve | Command::Keep) => {
                                if let Some(review) = &mut pending_review {
                                    if let Some(item) = review.next_consent().cloned() {
                                        let result = if action == Some(Command::Keep) {
                                            review.keep_path(&item.path)
                                        } else {
                                            review.approve_path(&item.path)
                                        };
                                        match result {
                                            Ok(()) => {
                                                confirm_move = false;
                                                review_text = Some(review.text(&running_status));
                                                status = if review.next_consent().is_some() { "Path reviewed. Review the next uncertain path." } else { "Folder review finished. Y/N chooses whether to move the listed items to Trash; Enter confirms." }.into();
                                                if review.is_empty() {
                                                    pending_review = None;
                                                    plan = None;
                                                    status = "All remaining data kept on disk. Nothing moved.".into();
                                                }
                                                cursor = 0;
                                                scroll = 0;
                                            }
                                            Err(e) => {
                                                status = format!("{e:#}");
                                                pending_review = None;
                                            }
                                        }
                                    }
                                }
                                input.clear();
                            }
                            Some(Command::Confirm) if pending_review.is_some() && !busy => {
                                let review = pending_review.take().expect("review checked");
                                let sender = tx.clone();
                                busy = true;
                                input.clear();
                                moving = true;
                                status = "Moving approved objects to macOS Trash…".into();
                                tokio::task::spawn_blocking(move || {
                                    let _ = sender.send(Message::Cleaned(review.execute()));
                                });
                            }
                            Some(Command::Remove) if plan.is_some() && !busy => {
                                cleanup_choice = false;
                                pending_review = None;
                                let snapshot = plan.as_ref().expect("plan checked").clone();
                                let sender = tx.clone();
                                busy = true;
                                status = "Preparing cleanup review…".into();
                                input.clear();
                                tokio::task::spawn_blocking(move || {
                                    let result = (|| {
                                        let review =
                                            crate::cleanup::review::Review::prepare(&snapshot)?;
                                        review.revalidate()?;
                                        let running = crate::platform::macos::running_status(
                                            &snapshot.application.path,
                                        );
                                        let text = review.text(&running);
                                        Ok((review, text))
                                    })();
                                    let _ = sender.send(Message::Review(result));
                                });
                            }
                            Some(Command::Help) => {
                                status = keyboard_help(&context);
                                input.clear();
                            }
                            Some(_) => {
                                status = "Action unavailable here. Press F1 for controls.".into();
                            }
                            None => {
                                status =
                                    "Unknown or unavailable action. Press F1 for controls.".into();
                            }
                        }
                    }
                    KeyCode::Char(' ')
                        if plan.is_some()
                            && input.is_empty()
                            && !show_issues
                            && review_text.is_none()
                            && !busy =>
                    {
                        if let Some(p) = &mut plan {
                            if let Some(f) = p.findings.get(cursor) {
                                if let Err(e) = p.select(cursor, !f.selected) {
                                    status = e.to_string();
                                }
                            }
                        }
                    }
                    KeyCode::Backspace => {
                        input.pop();
                        if plan.is_none() && !input.starts_with('/') {
                            search.clone_from(&input);
                            cursor = 0;
                            scroll = 0;
                        }
                    }
                    KeyCode::Char(c)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        input.push(c);
                        if plan.is_none() && !input.starts_with('/') {
                            search.clone_from(&input);
                            cursor = 0;
                            scroll = 0;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn permission_help() -> Vec<Line<'static>> {
    let step = |number: &'static str, text: &'static str| {
        Line::from(vec![
            Span::styled(number, Style::default().fg(theme::BLUE).bold()),
            Span::raw(text),
        ])
    };
    vec![
        Line::from(Span::styled(
            "This item cannot be verified or selected. Nothing was restored.",
            Style::default().fg(theme::TEXT).bold(),
        )),
        Line::from("macOS denied access; privacy restrictions or file permissions may apply."),
        Line::from(""),
        step("1. ", "Press S to open System Settings."),
        step("2. ", "Go to Privacy & Security → Full Disk Access."),
        step(
            "3. ",
            "Enable your host terminal/app (Terminal or iTerm), only if you trust it.",
        ),
        step(
            "4. ",
            "Restart that app if needed, reopen rmpkgx, then press F5 to check again.",
        ),
        Line::from(""),
        Line::from(Span::styled(
            "Full Disk Access is broad permission; macOS requires a manual grant.",
            Style::default().fg(theme::AMBER),
        )),
        Line::from("It may not fix this error. If still blocked, restore through Finder."),
        Line::from(Span::styled(
            "Recheck does not restore: selection and confirmation are still required.",
            Style::default().fg(theme::MUTED),
        )),
    ]
}

fn cleanup_menu_key(key: KeyCode, focused: &mut bool, review: &mut bool) -> bool {
    match key {
        KeyCode::Right if !*focused => {
            *focused = true;
            *review = false;
            true
        }
        KeyCode::Left | KeyCode::Esc if *focused => {
            *focused = false;
            *review = false;
            true
        }
        KeyCode::Up if *focused => {
            *review = false;
            true
        }
        KeyCode::Down if *focused => {
            *review = true;
            true
        }
        KeyCode::Char(' ') if *focused => true,
        _ => false,
    }
}
fn cleanup_menu(review: bool, selected_data: bool, focused: bool) -> Vec<Line<'static>> {
    let label = if selected_data {
        "Review cleanup"
    } else {
        "Review app only"
    };
    [("Back", !review), (label, review)]
        .into_iter()
        .map(|(label, selected)| {
            let active = focused && selected;
            Line::from(vec![
                Span::styled(
                    if active { "❯ " } else { "  " },
                    Style::default().fg(theme::ACCENT),
                ),
                Span::styled(
                    label,
                    if active {
                        Style::default().fg(theme::TEXT).bold()
                    } else {
                        Style::default().fg(theme::MUTED)
                    },
                ),
            ])
        })
        .collect()
}

fn dialogue_answer(key: KeyCode) -> Option<bool> {
    match key {
        KeyCode::Char('y' | 'Y') | KeyCode::Right => Some(true),
        KeyCode::Char('n' | 'N') | KeyCode::Left => Some(false),
        _ => None,
    }
}
fn decision_prompt(question: &str, yes: bool) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{question} "),
            Style::default().fg(theme::TEXT).bold(),
        ),
        Span::styled(
            if yes { "[Y/n] y" } else { "[y/N] n" },
            Style::default()
                .fg(if yes { theme::AMBER } else { theme::BLUE })
                .bold(),
        ),
    ])
}
fn ownership_choices(include: bool) -> Line<'static> {
    decision_prompt("Include this path in cleanup?", include)
}
fn ownership_document(text: &str, needs_approval: bool, include: bool) -> Vec<Line<'static>> {
    let mut result = vec![];
    for line in review_lines(text) {
        let question = line
            .to_string()
            .starts_with("Include this folder in cleanup,");
        result.push(line);
        if needs_approval && question {
            result.push(ownership_choices(include));
            result.push(Line::raw(""));
        }
    }
    result
}

/// Review and result text is a document, not a list of selectable rows.
fn review_lines(text: &str) -> Vec<Line<'static>> {
    text.lines()
        .map(|line| {
            let style = if line.starts_with("Path to decide:") {
                Style::default().fg(theme::BLUE).bold()
            } else if line.starts_with("Ownership decision")
                || line.starts_with("Planned paths")
                || line.starts_with("To move to Trash:")
                || line.starts_with("Could not fully check:")
            {
                Style::default().fg(theme::ACCENT).bold()
            } else if line.starts_with("Confidence:")
                || line.starts_with("Warning:")
                || line.starts_with("MOVED — VERIFICATION")
            {
                Style::default().fg(theme::AMBER)
            } else if line.starts_with("FAILED") || line.starts_with("SKIPPED") {
                Style::default().fg(theme::RED)
            } else if line.starts_with("MOVED ") {
                Style::default().fg(theme::GREEN)
            } else {
                Style::default().fg(theme::MUTED)
            };
            Line::from(Span::styled(line.to_owned(), style))
        })
        .collect()
}

fn row(selected: bool, mut spans: Vec<Span<'static>>) -> Line<'static> {
    spans.insert(
        0,
        Span::styled(
            if selected { "❯ " } else { "  " },
            Style::default().fg(theme::ACCENT).bold(),
        ),
    );
    Line::from(spans).style(if selected {
        Style::default()
            .bg(theme::PANEL)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Open,
    Back,
    Errors,
    Files,
    Cancel,
    Help,
    Quit,
    Select,
    Unselect,
    Remove,
    Confirm,
    Approve,
    Keep,
}
const COMMANDS: &[(&str, Command)] = &[
    ("/select", Command::Select),
    ("/unselect", Command::Unselect),
    ("/remove", Command::Remove),
    ("/confirm", Command::Confirm),
    ("/approve", Command::Approve),
    ("/keep", Command::Keep),
    ("/open", Command::Open),
    ("/back", Command::Back),
    ("/errors", Command::Errors),
    ("/files", Command::Files),
    ("/cancel", Command::Cancel),
    ("/help", Command::Help),
    ("/quit", Command::Quit),
];
fn parse_command(input: &str) -> Option<Command> {
    COMMANDS
        .iter()
        .find(|(name, _)| *name == input.trim())
        .map(|(_, action)| *action)
}
#[derive(Default)]
struct CommandContext {
    cleanup_menu_focused: bool,
    restore_access_denied: bool,
    restore_list: bool,
    restore_confirmation: bool,
    recovery_empty: bool,
    history_retry: bool,
    history_origin: bool,
    history_list: bool,
    history_detail: bool,
    busy: bool,
    moving: bool,
    has_plan: bool,
    review: bool,
    has_pending_review: bool,
    needs_approval: bool,
    show_issues: bool,
    has_issues: bool,
    can_select: bool,
    can_unselect: bool,
    has_apps: bool,
}
impl CommandContext {
    fn available(&self, command: Command) -> bool {
        if command == Command::Help {
            return true;
        }
        if self.moving {
            return false;
        }
        if command == Command::Quit {
            return true;
        }
        if self.busy {
            return command == Command::Cancel && !self.has_plan && !self.review;
        }
        if self.restore_list || self.restore_confirmation {
            return false;
        }
        if self.review {
            return match command {
                Command::Back => true,
                Command::Files => self.has_plan,
                Command::Approve | Command::Keep => self.has_pending_review && self.needs_approval,
                Command::Confirm => self.has_pending_review && !self.needs_approval,
                _ => false,
            };
        }
        if !self.has_plan {
            return command == Command::Open && self.has_apps;
        }
        match command {
            Command::Back => true,
            Command::Files => self.show_issues,
            Command::Errors => !self.show_issues && self.has_issues,
            Command::Remove => !self.show_issues && !self.recovery_empty,
            Command::Select => !self.show_issues && self.can_select,
            Command::Unselect => !self.show_issues && self.can_unselect,
            _ => false,
        }
    }
}

fn control_hints(context: &CommandContext, empty_input: bool) -> Line<'static> {
    let mut spans = vec![];
    let mut add = |key: &str, action: &str, color: Color| {
        if !spans.is_empty() {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            format!(" {key} "),
            Style::default().fg(color).bg(theme::PANEL).bold(),
        ));
        spans.push(Span::styled(
            format!(" {action}"),
            Style::default().fg(theme::TEXT),
        ));
    };
    if context.moving {
        add(
            "…",
            "Moving approved objects; wait for report",
            theme::AMBER,
        );
    } else if context.busy {
        if context.available(Command::Cancel) {
            add("Esc", "stop scan", theme::AMBER);
        }
        add("F1", "help", theme::ACCENT);
    } else if context.restore_confirmation {
        add("Y / N", "answer", theme::ACCENT);
        add("Enter", "confirm", theme::GREEN);
        add("Esc", "cancel", theme::ACCENT);
    } else if context.restore_access_denied {
        add("S", "open Settings", theme::BLUE);
        add("F5", "check again", theme::GREEN);
        add("↑ ↓", "other objects", theme::ACCENT);
        add("Esc", "history", theme::ACCENT);
    } else if context.restore_list {
        add("F5", "check again", theme::BLUE);
        add("↑ ↓", "navigate", theme::ACCENT);
        add("Space", "select", theme::GREEN);
        add("R", "review restore", theme::GREEN);
        add("Esc", "history", theme::ACCENT);
    } else if context.history_detail {
        add("↑ ↓", "scroll journal", theme::ACCENT);
        if context.history_retry {
            add("Enter", "re-scan remaining data", theme::GREEN);
        }
        add("F3", "restore from Trash", theme::BLUE);
        add("Esc", "history", theme::ACCENT);
    } else if context.history_list {
        add("↑ ↓", "navigate", theme::ACCENT);
        if context.has_apps {
            add("Enter", "inspect journal", theme::GREEN);
        }
        add("F2 / Esc", "applications", theme::BLUE);
    } else if !empty_input && context.has_plan {
        add("Backspace", "edit", theme::ACCENT);
        add("Esc", "clear input", theme::ACCENT);
    } else if context.needs_approval {
        add("Y / N", "answer", theme::ACCENT);
        add("Enter", "apply", theme::GREEN);
        add("Esc", "cancel review", theme::ACCENT);
    } else if context.available(Command::Confirm) {
        add("Y / N", "answer", theme::ACCENT);
        add("Enter", "confirm", theme::GREEN);
        add("↑ ↓", "scroll plan", theme::ACCENT);
        add("Esc", "cancel", theme::ACCENT);
    } else if context.cleanup_menu_focused && context.available(Command::Remove) {
        add("↑ ↓", "choose action", theme::ACCENT);
        add("Enter", "open choice", theme::GREEN);
        add("← / Esc", "files", theme::ACCENT);
    } else if context.review {
        add("↑ ↓", "scroll", theme::ACCENT);
        add(
            "Enter / Esc",
            if context.history_origin {
                "back to history"
            } else {
                "back to apps"
            },
            theme::ACCENT,
        );
    } else {
        add("↑ ↓", "navigate", theme::ACCENT);
        if !context.has_plan {
            if context.has_apps {
                add("Enter", "inspect", theme::GREEN);
            }
            if !empty_input {
                add("Esc", "clear search", theme::ACCENT);
            }
        } else {
            if !context.show_issues && (context.can_select || context.can_unselect) {
                add("Space", "select", theme::GREEN);
            }
            if context.available(Command::Remove) {
                add("→", "actions", theme::ACCENT);
                add("Enter", "open choice", theme::GREEN);
            }
            if context.has_issues || context.show_issues {
                add(
                    "Tab",
                    if context.show_issues {
                        "files"
                    } else {
                        "errors"
                    },
                    theme::BLUE,
                );
            }
            add("Esc", "back", theme::ACCENT);
        }
        if !context.has_plan {
            add("F2", "history", theme::BLUE);
            add("F4", "leftovers", theme::BLUE);
        }
        add("F1", "help", theme::ACCENT);
        add("Ctrl+C", "quit", theme::ACCENT);
    }
    Line::from(spans)
}

fn keyboard_help(context: &CommandContext) -> String {
    let exit = if context.moving {
        ""
    } else {
        " · Ctrl+C quit"
    };
    format!("{} · F1 help{exit}", navigation_hints(context))
}

fn navigation_hints(context: &CommandContext) -> &'static str {
    if context.restore_access_denied {
        return "S open Settings · F5 recheck (no automatic restore) · Esc history";
    }
    if context.restore_confirmation {
        return "Y/N answer · Enter confirm restore · Esc back";
    }
    if context.restore_list {
        return "↑↓ navigate · Space select · R review restoration · F5 recheck · Esc history";
    }
    if context.history_list {
        return "↑↓ journals · Enter inspect · F2/Esc applications";
    }
    if context.history_detail {
        return if context.history_retry {
            "↑↓ scroll · Enter re-scan known leftovers · Esc history"
        } else {
            "↑↓ scroll · Esc history"
        };
    }
    if context.busy {
        if context.available(Command::Cancel) {
            "Esc cancel scan"
        } else {
            "Please wait for the operation to finish"
        }
    } else if context.needs_approval {
        "Y/N answer · Enter apply · Esc back"
    } else if context.available(Command::Confirm) {
        "←→ choose · Enter confirm · Esc back"
    } else if context.review {
        "↑↓ scroll · Enter or Esc back"
    } else if context.cleanup_menu_focused && context.available(Command::Remove) {
        "↑↓ choose action · Enter open choice · ←/Esc files"
    } else if context.has_plan && context.show_issues {
        "↑↓ navigate · Tab files · Esc back"
    } else if context.has_plan && context.has_issues {
        "↑↓ navigate · Space toggle · → actions · Enter open choice · Tab errors · Esc back"
    } else if context.has_plan {
        "↑↓ navigate · Space toggle · → actions · Enter open choice · Esc back"
    } else {
        "Type to search · ↑↓ navigate · Enter inspect · F2 history · F4 leftovers"
    }
}
fn navigation_key(
    key: KeyCode,
    context: &CommandContext,
    confirm_move: bool,
) -> Option<&'static str> {
    if key == KeyCode::Esc && context.available(Command::Cancel) {
        return Some("/cancel");
    }
    if context.busy || (context.history_detail && key == KeyCode::Enter) {
        return None;
    }
    // Y/N changes the answer only; Enter is always required to apply it.
    if dialogue_answer(key).is_some()
        && (context.needs_approval || context.available(Command::Confirm))
    {
        return None;
    }
    match key {
        KeyCode::Enter if context.available(Command::Remove) => {
            Some(if confirm_move { "/remove" } else { "/back" })
        }
        KeyCode::Enter if context.needs_approval => {
            Some(if confirm_move { "/approve" } else { "/keep" })
        }
        KeyCode::Enter if context.available(Command::Confirm) => {
            Some(if confirm_move { "/confirm" } else { "/files" })
        }
        KeyCode::Enter if context.review && !context.has_pending_review => Some("/back"),
        KeyCode::Esc if context.review => Some(if context.has_plan { "/files" } else { "/back" }),
        KeyCode::Esc if context.has_plan => Some(if context.show_issues {
            "/files"
        } else {
            "/back"
        }),
        KeyCode::Char('r' | 'R') if context.available(Command::Remove) => Some("/remove"),
        KeyCode::Tab if context.available(Command::Errors) => Some("/errors"),
        KeyCode::Tab if context.show_issues && context.available(Command::Files) => Some("/files"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commands_require_exact_names_and_do_not_consume_searches() {
        assert_eq!(parse_command("/quit"), Some(Command::Quit));
        assert_eq!(parse_command("/confirm"), Some(Command::Confirm));
        assert_eq!(parse_command("/approve"), Some(Command::Approve));
        for text in ["q", "Chrome", "/q", "/quit extra", "/confirm extra"] {
            assert_eq!(parse_command(text), None);
        }
    }
    #[test]
    fn application_list_hides_details_and_confirmation_commands() {
        let context = CommandContext {
            has_apps: true,
            ..Default::default()
        };
        let hints = navigation_hints(&context);
        assert!(!hints.contains("/open"));
        assert!(!hints.contains("/quit"));
        for command in [
            Command::Remove,
            Command::Select,
            Command::Unselect,
            Command::Back,
            Command::Errors,
            Command::Files,
            Command::Confirm,
            Command::Approve,
            Command::Cancel,
        ] {
            assert!(!context.available(command));
        }
    }
    #[test]
    fn details_and_errors_expose_only_applicable_actions() {
        let context = CommandContext {
            has_plan: true,
            can_select: true,
            has_issues: true,
            ..Default::default()
        };
        assert!(context.available(Command::Select));
        assert!(!context.available(Command::Unselect));
        assert!(context.available(Command::Errors));
        assert!(!context.available(Command::Files));
        assert!(!context.available(Command::Open));
        assert!(!context.available(Command::Confirm));
        assert!(!navigation_hints(&context).contains("/errors"));
        assert_eq!(
            navigation_key(KeyCode::Tab, &context, false),
            Some("/errors")
        );
        assert!(!navigation_hints(&context).contains("/quit"));
        let errors = CommandContext {
            show_issues: true,
            ..context
        };
        assert!(errors.available(Command::Files));
        assert!(!errors.available(Command::Select));
        assert!(!errors.available(Command::Remove));
    }
    #[test]
    fn approval_confirmation_and_busy_states_are_distinct() {
        let mut context = CommandContext {
            review: true,
            has_plan: true,
            has_pending_review: true,
            needs_approval: true,
            ..Default::default()
        };
        assert!(context.available(Command::Approve));
        assert!(!context.available(Command::Confirm));
        assert!(!context.available(Command::Remove));
        context.needs_approval = false;
        assert!(!context.available(Command::Approve));
        assert!(context.available(Command::Confirm));
        context.has_pending_review = false;
        assert!(!context.available(Command::Confirm));
        let scan = CommandContext {
            busy: true,
            ..Default::default()
        };
        assert!(scan.available(Command::Cancel));
        assert!(!scan.available(Command::Open));
        let moving = CommandContext {
            moving: true,
            busy: true,
            ..Default::default()
        };
        assert!(!moving.available(Command::Cancel));
        assert!(!moving.available(Command::Quit));
        assert!(moving.available(Command::Help));
    }
    #[test]
    fn review_keys_apply_only_to_a_pending_uncertain_path() {
        let context = CommandContext {
            review: true,
            has_pending_review: true,
            needs_approval: true,
            ..Default::default()
        };
        assert_eq!(dialogue_answer(KeyCode::Char('y')), Some(true));
        assert_eq!(dialogue_answer(KeyCode::Char('N')), Some(false));
        assert_eq!(navigation_key(KeyCode::Char('y'), &context, false), None);
        assert_eq!(
            navigation_key(KeyCode::Enter, &context, true),
            Some("/approve")
        );
        assert_eq!(
            navigation_key(KeyCode::Enter, &context, false),
            Some("/keep")
        );
    }
    #[test]
    fn keyboard_navigation_defaults_to_cancel_and_returns_from_results() {
        let review = CommandContext {
            review: true,
            has_plan: true,
            has_pending_review: true,
            ..Default::default()
        };
        assert_eq!(
            navigation_key(KeyCode::Enter, &review, false),
            Some("/files")
        );
        assert_eq!(
            navigation_key(KeyCode::Enter, &review, true),
            Some("/confirm")
        );
        assert_eq!(navigation_key(KeyCode::Esc, &review, true), Some("/files"));
        let results = CommandContext {
            review: true,
            ..Default::default()
        };
        assert_eq!(
            navigation_key(KeyCode::Enter, &results, false),
            Some("/back")
        );
        assert_eq!(navigation_key(KeyCode::Esc, &results, false), Some("/back"));
        assert!(!navigation_hints(&review).contains("/confirm"));
        let busy = CommandContext {
            busy: true,
            ..review
        };
        assert_eq!(navigation_key(KeyCode::Enter, &busy, true), None);
    }
    #[test]
    fn cleanup_key_only_prepares_a_review_on_the_files_screen() {
        let details = CommandContext {
            has_plan: true,
            ..Default::default()
        };
        for key in [KeyCode::Char('r'), KeyCode::Char('R')] {
            assert_eq!(navigation_key(key, &details, false), Some("/remove"));
            assert_eq!(navigation_key(key, &CommandContext::default(), false), None);
            assert_eq!(
                navigation_key(
                    key,
                    &CommandContext {
                        show_issues: true,
                        has_plan: true,
                        ..Default::default()
                    },
                    false
                ),
                None
            );
            assert_eq!(
                navigation_key(
                    key,
                    &CommandContext {
                        review: true,
                        has_pending_review: true,
                        ..Default::default()
                    },
                    true
                ),
                None
            );
        }
    }
    #[test]
    fn review_document_has_no_selection_marker_and_emphasizes_only_the_decision() {
        let lines = review_lines("Ownership decision\nPath to decide: /fixture/data\nPlanned paths (reference only):\n  /fixture/app\n");
        assert!(lines.iter().all(|line| !line.to_string().contains('❯')));
        assert_eq!(lines[1].spans[0].style.fg, Some(theme::BLUE));
        assert_eq!(lines[3].spans[0].style.fg, Some(theme::MUTED));
        assert_eq!(lines[3].spans[0].style.bg, None);
    }
    #[test]
    fn ownership_decision_shows_explicit_choices_and_enter_defaults_to_keep() {
        let context = CommandContext {
            review: true,
            has_plan: true,
            has_pending_review: true,
            needs_approval: true,
            ..Default::default()
        };
        assert_eq!(
            navigation_key(KeyCode::Enter, &context, false),
            Some("/keep")
        );
        assert_eq!(
            navigation_key(KeyCode::Enter, &context, true),
            Some("/approve")
        );
        let text = "Path to decide: /fixture/data\nInclude this folder in cleanup, or keep it on disk?\nReference information";
        let keep = ownership_document(text, true, false);
        assert_eq!(keep.len(), 5);
        assert!(keep[2].to_string().contains("[y/N]"));
        assert!(!keep[2].to_string().contains("[Y/n]"));
        assert!(ownership_choices(true).to_string().contains("[Y/n]"));
        assert_eq!(ownership_document(text, false, false).len(), 3);
    }
    #[test]
    fn details_menu_defaults_to_back_and_only_opens_a_review() {
        let details = CommandContext {
            has_plan: true,
            can_select: true,
            ..Default::default()
        };
        assert_eq!(
            navigation_key(KeyCode::Enter, &details, false),
            Some("/back")
        );
        assert_eq!(
            navigation_key(KeyCode::Enter, &details, true),
            Some("/remove")
        );
        let menu = cleanup_menu(false, true, true);
        assert_eq!(menu.len(), 2);
        assert_eq!(menu[0].to_string(), "❯ Back");
        assert_eq!(menu[1].to_string(), "  Review cleanup");
        assert_eq!(
            cleanup_menu(true, false, true)[1].to_string(),
            "❯ Review app only"
        );
        assert!(cleanup_menu(false, true, false)
            .iter()
            .all(|line| !line.to_string().contains('❯')));
        for line in menu {
            for span in line.spans {
                assert_eq!(span.style.bg, None);
            }
        }
    }
    #[test]
    fn history_keyboard_help_distinguishes_listing_from_recovery() {
        let list = CommandContext {
            history_list: true,
            has_apps: true,
            ..Default::default()
        };
        let detail = CommandContext {
            history_detail: true,
            history_retry: true,
            review: true,
            ..Default::default()
        };
        assert!(navigation_hints(&list).contains("Enter inspect"));
        assert!(navigation_hints(&detail).contains("re-scan"));
        assert!(!list.available(Command::Confirm));
        assert!(!detail.available(Command::Confirm));
        assert!(!detail.available(Command::Remove));
    }
    #[test]
    fn empty_recovery_selection_has_no_app_only_cleanup_action() {
        let context = CommandContext {
            has_plan: true,
            recovery_empty: true,
            ..Default::default()
        };
        assert!(!context.available(Command::Remove));
        assert_eq!(navigation_key(KeyCode::Char('r'), &context, true), None);
        assert_eq!(navigation_key(KeyCode::Enter, &context, true), None);
    }
    #[test]
    fn access_help_panel_fits_a_standard_terminal_and_keeps_the_consent_notice() {
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 13)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(
                    Paragraph::new(permission_help())
                        .block(Block::default().borders(ratatui::widgets::Borders::ALL))
                        .wrap(Wrap { trim: true }),
                    frame.area(),
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("Press S to open System Settings"));
        assert!(text.contains("manual grant"));
        assert!(text.contains("selection and confirmation are still required"));
    }
    #[test]
    fn permission_settings_action_is_only_advertised_for_denied_restore_entries() {
        let denied = CommandContext {
            restore_list: true,
            restore_access_denied: true,
            ..Default::default()
        };
        let hints = control_hints(&denied, true).to_string();
        assert!(hints.contains("open Settings"));
        assert!(hints.contains("check again"));
        assert!(!hints.contains("review restore"));
        let normal = CommandContext {
            restore_list: true,
            ..Default::default()
        };
        assert!(!control_hints(&normal, true)
            .to_string()
            .contains("open Settings"));
    }
    #[test]
    fn leftovers_hint_is_only_on_the_application_screen_and_report_has_no_actions() {
        let root = CommandContext::default();
        assert!(control_hints(&root, true).to_string().contains("F4"));
        for context in [
            CommandContext {
                has_plan: true,
                ..Default::default()
            },
            CommandContext {
                review: true,
                ..Default::default()
            },
        ] {
            let hints = control_hints(&context, true).to_string();
            assert!(!hints.contains("F4"));
        }
        let report = CommandContext {
            review: true,
            ..Default::default()
        };
        assert!(!report.available(Command::Remove));
        assert!(!report.available(Command::Select));
        assert!(navigation_hints(&report).contains("scroll"));
    }
    #[test]
    fn vertical_cleanup_menu_keeps_file_navigation_until_explicit_focus() {
        let (mut focused, mut review) = (false, false);
        assert!(!cleanup_menu_key(KeyCode::Down, &mut focused, &mut review));
        assert!(cleanup_menu_key(KeyCode::Right, &mut focused, &mut review));
        assert!(focused && !review);
        assert!(cleanup_menu_key(KeyCode::Down, &mut focused, &mut review));
        assert!(review);
        assert!(!cleanup_menu_key(KeyCode::Enter, &mut focused, &mut review));
        assert!(cleanup_menu_key(KeyCode::Esc, &mut focused, &mut review));
        assert!(!focused && !review);
        assert!(!cleanup_menu_key(KeyCode::Up, &mut focused, &mut review));
    }
    #[test]
    fn restore_list_distinguishes_review_from_final_confirmation() {
        let list = CommandContext {
            restore_list: true,
            ..Default::default()
        };
        let hints = control_hints(&list, true).to_string();
        assert!(hints.contains(" R "));
        assert!(hints.contains("F5"));
        assert!(!hints.contains("Enter"));
        assert!(navigation_hints(&list).contains("R review"));
        let confirm = CommandContext {
            restore_confirmation: true,
            ..Default::default()
        };
        assert!(control_hints(&confirm, true).to_string().contains("Enter"));
    }
}
