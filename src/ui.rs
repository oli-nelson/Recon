use crate::{
    Config,
    animation::{self, FramePacer},
    keybinds::Action,
    model::{Command, Model, Status},
    watch::Worker,
};
use anyhow::Result;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{
        BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
        disable_raw_mode, enable_raw_mode,
    },
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, HighlightSpacing, List, ListItem, ListState, Paragraph,
        Widget,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, IsTerminal},
    time::{Instant, UNIX_EPOCH},
};

#[derive(Default)]
pub struct View {
    pub filter: Option<String>,
    pub selected: Option<String>,
    pub collapsed: BTreeSet<String>,
    pub sidebar: bool,
    pub panel_focus: bool,
    pub following: bool,
    pub scroll: usize,
    pub cursor: usize,
    pub help: bool,
    pub unseen: usize,
    pub tree_offset: usize,
    anchor: Option<(u64, usize)>,
    started: Option<Instant>,
    pub tree_percent: u16,
    arrivals: BTreeMap<String, Instant>,
    divider: Option<u16>,
    layout_x: u16,
    layout_width: u16,
    dragging: bool,
    split_area: Rect,
    pub search: String,
    pub searching: bool,
    search_backup: Option<SearchBackup>,
}
impl View {
    pub fn new() -> Self {
        Self {
            tree_percent: 32,
            started: Some(Instant::now()),
            sidebar: true,
            following: true,
            ..Self::default()
        }
    }
}
struct SearchBackup {
    query: String,
    selected: Option<String>,
    filter: Option<String>,
    scroll: usize,
    following: bool,
    focus: bool,
    tree_offset: usize,
}
#[derive(Clone)]
struct TreeRow {
    key: String,
    dir: bool,
    depth: usize,
}
fn tree(model: &Model, view: &View) -> Vec<TreeRow> {
    let mut nodes = BTreeSet::new();
    let query = view.search.to_lowercase();
    for path in model.files.keys() {
        if !query.is_empty() && !path.to_lowercase().contains(&query) {
            continue;
        }
        let mut dir = String::new();
        let parts: Vec<_> = path.split('/').collect();
        for p in &parts[..parts.len() - 1] {
            if !dir.is_empty() {
                dir.push('/');
            }
            dir.push_str(p);
            nodes.insert((format!("{dir}/"), true));
        }
        nodes.insert((path.clone(), false));
    }
    nodes
        .into_iter()
        .filter(|(p, _)| {
            !view.search.is_empty() || !view.collapsed.iter().any(|d| p != d && p.starts_with(d))
        })
        .map(|(key, dir)| TreeRow {
            depth: key.trim_end_matches('/').matches('/').count(),
            key,
            dir,
        })
        .collect()
}
fn length(entry: &crate::model::Entry) -> usize {
    if entry.collapsed {
        2
    } else {
        2 + entry.line_count.max(1)
    }
}
fn entry_matches(entry: &crate::model::Entry, view: &View) -> bool {
    view.filter.as_ref().is_none_or(|path| *path == entry.path)
        && (view.search.is_empty()
            || entry
                .path
                .to_lowercase()
                .contains(&view.search.to_lowercase()))
}

fn total(model: &Model, view: &View) -> usize {
    model
        .entries
        .iter()
        .filter(|e| entry_matches(e, view))
        .map(length)
        .sum()
}
fn save_anchor(model: &Model, view: &mut View) {
    let mut row = 0;
    for e in model
        .entries
        .iter()
        .rev()
        .filter(|e| entry_matches(e, view))
    {
        let n = length(e);
        if view.scroll < row + n {
            view.anchor = Some((e.id, view.scroll - row));
            return;
        }
        row += n;
    }
    view.anchor = None;
}
fn restore_anchor(model: &Model, view: &mut View) {
    let Some((id, offset)) = view.anchor else {
        return;
    };
    let mut row = 0;
    for e in model
        .entries
        .iter()
        .rev()
        .filter(|e| entry_matches(e, view))
    {
        if e.id == id {
            view.scroll = row + offset.min(length(e) - 1);
            return;
        }
        row += length(e);
    }
    // If the anchored entry was evicted, stay near the oldest retained content.
    view.scroll = total(model, view).saturating_sub(1);
}

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            EndSynchronizedUpdate,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
    }
}
pub fn run(config: &Config, worker: Worker) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        anyhow::bail!("Interactive mode requires a terminal; use --once for a snapshot");
    }
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut model = Model::new(
        config.history_entries as usize,
        config.history_bytes as usize,
    );
    let mut view = View::new();
    view.tree_percent = 100 - config.sidebar_width;
    let mut dirty = true;
    let mut pacer = FramePacer::new(config.fps, Instant::now());
    let mut was_active = false;
    let result = (|| -> Result<()> {
        loop {
            let before = model.entries.back().map(|e| e.id);
            save_anchor(&model, &mut view);
            let mut updated = false;
            for update in worker.updates.try_iter().take(16) {
                updated = true;
                if matches!(update, crate::model::Update::Reset { .. }) {
                    view.filter = None;
                    view.scroll = 0;
                    view.following = true;
                    view.anchor = None;
                    view.unseen = 0;
                    view.arrivals.clear();
                }
                if let crate::model::Update::Files(rows) = &update {
                    for (row, _) in rows {
                        if !config.no_animations
                            && !config.reduced_motion
                            && !row.remove_from_tree
                            && !model.files.contains_key(&row.path)
                        {
                            view.arrivals.insert(row.path.clone(), Instant::now());
                        }
                    }
                }
                model.apply(update);
                dirty = true;
            }
            if updated && !view.search.is_empty() {
                search_selection(&model, &mut view);
            }
            if !view.following {
                restore_anchor(&model, &mut view);
            }
            let added = model.entries.back().map_or(0, |entry| {
                before.map_or(model.entries.len(), |id| {
                    entry.id.saturating_sub(id) as usize
                })
            });
            if !view.following || !view.sidebar {
                view.unseen += added;
            }
            let duration = std::time::Duration::from_millis(config.animation_ms);
            view.arrivals
                .retain(|path, start| start.elapsed() < duration && model.files.contains_key(path));
            let active = !config.no_animations
                && model
                    .files
                    .values()
                    .any(|r| r.activity.elapsed() < duration)
                || (!config.no_animations
                    && view.sidebar
                    && model
                        .entries
                        .back()
                        .is_some_and(|e| e.time.elapsed().is_ok_and(|age| age < duration)));
            let active = active || (!config.no_animations && !view.arrivals.is_empty());
            let notice_visible = model.notice_remaining(Instant::now()).is_some()
                && !view.searching
                && view.search.is_empty();
            let animation = (active || model.refreshing || notice_visible)
                && !config.reduced_motion
                && !config.no_animations;
            if was_active && !active {
                dirty = true;
            }
            was_active = active;
            dirty |= animation;
            let now = Instant::now();
            dirty |= model.expire_notice(now);
            if dirty && pacer.ready(now) {
                execute!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
                let rendered = terminal
                    .draw(|f| draw(f, &model, &mut view, config))
                    .map(|_| ());
                let ended = execute!(terminal.backend_mut(), EndSynchronizedUpdate);
                rendered?;
                ended?;
                pacer.presented(now);
                dirty = false;
            }
            if event::poll(pacer.wait(Instant::now(), dirty || animation))? {
                let input = event::read()?;
                if matches!(&input, Event::Key(_) | Event::Mouse(_) | Event::Paste(_)) {
                    dirty |= model.dismiss_notice();
                }
                match input {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        let height = terminal.size()?.height as usize;
                        let key = if view.searching || view.help {
                            Some(key)
                        } else {
                            config.keybinds.resolve(key)
                        };
                        if key.is_some_and(|key| {
                            handle_key(key, &mut model, &mut view, &worker.commands, height)
                        }) {
                            break;
                        }
                        dirty = true;
                    }
                    Event::Mouse(mouse) => {
                        dirty |= handle_mouse(mouse, &mut view);
                    }
                    Event::Resize(_, _) => {
                        view.dragging = false;
                        view.divider = None;
                        dirty = true;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    })();
    worker.stop();
    terminal.show_cursor()?;
    result
}
fn resize_split(view: &mut View, percent: u16) {
    let width = view.layout_width.max(72) as u32;
    let min = (18 * 100u32).div_ceil(width).max(10) as u16;
    let max = (100 - (24 * 100u32).div_ceil(width)).min(90) as u16;
    view.tree_percent = percent.clamp(min, max);
}
fn handle_mouse(mouse: MouseEvent, view: &mut View) -> bool {
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            view.dragging = view.sidebar
                && view.divider.is_some_and(|x| x.abs_diff(mouse.column) <= 1)
                && mouse.row >= view.split_area.y
                && mouse.row < view.split_area.bottom()
                && !view.help;
            view.dragging
        }
        MouseEventKind::Drag(MouseButton::Left) if view.dragging && view.divider.is_some() => {
            let relative = mouse.column.saturating_sub(view.layout_x) as u32;
            let percent = ((relative * 100) / view.layout_width.max(1) as u32).min(100) as u16;
            resize_split(view, percent);
            true
        }
        MouseEventKind::Up(MouseButton::Left) => {
            let was_dragging = view.dragging;
            view.dragging = false;
            was_dragging
        }
        _ => false,
    }
}

fn search_selection(model: &Model, view: &mut View) {
    let rows = tree(model, view);
    if view
        .selected
        .as_ref()
        .is_some_and(|selected| rows.iter().any(|r| !r.dir && &r.key == selected))
    {
        return;
    }
    if let Some(row) = rows.iter().find(|r| !r.dir) {
        select(view, row);
    } else {
        view.selected = None;
        view.filter = None;
    }
}

fn handle_key(
    key: KeyEvent,
    model: &mut Model,
    view: &mut View,
    commands: &std::sync::mpsc::SyncSender<Command>,
    height: usize,
) -> bool {
    use KeyCode::*;
    let page = height.saturating_sub(5).max(1);
    if view.searching {
        match key.code {
            Esc => {
                view.searching = false;
                if let Some(old) = view.search_backup.take() {
                    view.search = old.query;
                    view.selected = old.selected;
                    view.filter = old.filter;
                    view.scroll = old.scroll;
                    view.following = old.following;
                    view.panel_focus = old.focus;
                    view.tree_offset = old.tree_offset;
                }
            }
            Enter => {
                view.searching = false;
                view.search_backup = None;
            }
            Backspace => {
                view.search.pop();
                search_selection(model, view);
            }
            Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                view.search.clear();
                search_selection(model, view);
            }
            Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                view.search.push(c);
                search_selection(model, view);
            }
            _ => {}
        }
        return false;
    }
    if view.help {
        view.help = false;
        return false;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if view.panel_focus && matches!(key.code, Char('d') | Char('u')) {
            view.following = false;
            let amount = page.div_ceil(2);
            if key.code == Char('d') {
                view.scroll = (view.scroll + amount).min(total(model, view).saturating_sub(page));
            } else {
                view.scroll = view.scroll.saturating_sub(amount);
            }
        }
        return false;
    }
    match key.code {
        Char('/') => {
            view.search_backup = Some(SearchBackup {
                query: view.search.clone(),
                selected: view.selected.clone(),
                filter: view.filter.clone(),
                scroll: view.scroll,
                following: view.following,
                focus: view.panel_focus,
                tree_offset: view.tree_offset,
            });
            view.searching = true;
            view.panel_focus = false;
        }
        Char('n') | Char('N') if !view.search.is_empty() => {
            let rows: Vec<_> = tree(model, view).into_iter().filter(|r| !r.dir).collect();
            if !rows.is_empty() {
                let current = rows
                    .iter()
                    .position(|r| Some(&r.key) == view.selected.as_ref())
                    .unwrap_or(0);
                let next = if key.code == Char('N') {
                    (current + rows.len() - 1) % rows.len()
                } else {
                    (current + 1) % rows.len()
                };
                select(view, &rows[next]);
            }
        }
        Char('q') => return true,
        Char('c') | Char('g') => {
            let command = if key.code == Char('c') {
                Command::Clear
            } else {
                Command::Git
            };
            if commands.try_send(command).is_ok() {
                model.refreshing = true;
                model.message = "Requested baseline refresh…".into();
            } else {
                model.message = "Refresh queue busy; retry shortly".into();
            }
        }
        Char('d') => {
            view.dragging = false;
            view.sidebar = !view.sidebar;
            view.divider = None;
            if !view.sidebar {
                view.panel_focus = false;
            } else if view.following {
                view.unseen = 0;
            }
        }
        Char('[') => resize_split(view, view.tree_percent.saturating_sub(5)),
        Char(']') => resize_split(view, view.tree_percent.saturating_add(5)),
        Tab if view.sidebar => view.panel_focus = !view.panel_focus,
        Char('?') => view.help = true,
        Esc => {
            view.search.clear();
            view.filter = None;
            view.following = true;
            view.scroll = 0;
            view.unseen = 0;
        }
        Char('f') => {
            view.following = !view.following;
            if view.following {
                view.unseen = 0;
            }
        }
        PageUp | PageDown => {
            view.following = false;
            if key.code == PageUp {
                view.scroll = view.scroll.saturating_sub(page);
            } else {
                view.scroll = (view.scroll + page).min(total(model, view).saturating_sub(page));
            }
        }
        Enter if view.panel_focus => {
            let mut offset = 0;
            for e in model
                .entries
                .iter_mut()
                .rev()
                .filter(|e| entry_matches(e, view))
            {
                let len = length(e);
                if view.scroll + view.cursor < offset + len {
                    e.collapsed = !e.collapsed;
                    view.following = false;
                    break;
                }
                offset += len;
            }
        }
        Char('j') | Down | Char('k') | Up => {
            let down = matches!(key.code, Char('j') | Down);
            if view.panel_focus {
                view.following = false;
                if down {
                    view.scroll = (view.scroll + 1).min(total(model, view).saturating_sub(page));
                } else {
                    view.scroll = view.scroll.saturating_sub(1);
                }
            } else {
                let rows = tree(model, view);
                if !rows.is_empty() {
                    let index = view
                        .selected
                        .as_ref()
                        .and_then(|p| rows.iter().position(|r| &r.key == p))
                        .unwrap_or(if down { usize::MAX } else { 0 });
                    let index = if down {
                        if index == usize::MAX {
                            0
                        } else {
                            (index + 1).min(rows.len() - 1)
                        }
                    } else {
                        index.saturating_sub(1)
                    };
                    select(view, &rows[index]);
                }
            }
        }
        Char('h') | Left | Char('l') | Right | Enter => {
            let rows = tree(model, view);
            if let Some(index) = view
                .selected
                .as_ref()
                .and_then(|p| rows.iter().position(|r| &r.key == p))
            {
                let row = &rows[index];
                let expand = matches!(key.code, Char('l') | Right)
                    || key.code == Enter && view.collapsed.contains(&row.key);
                if key.code == Enter && !row.dir {
                    if view.filter.as_deref() != Some(&row.key) {
                        select(view, row);
                    }
                    view.sidebar = true;
                    view.panel_focus = true;
                } else if row.dir {
                    if expand {
                        view.collapsed.remove(&row.key);
                    } else {
                        view.collapsed.insert(row.key.clone());
                    }
                } else if !expand {
                    if let Some((parent, _)) = row.key.rsplit_once('/') {
                        view.selected = Some(format!("{parent}/"));
                        view.collapsed.insert(format!("{parent}/"));
                    }
                }
            }
        }
        _ => {}
    }
    false
}
fn select(view: &mut View, row: &TreeRow) {
    view.selected = Some(row.key.clone());
    if !row.dir {
        view.filter = Some(row.key.clone());
        view.following = true;
        view.scroll = 0;
        view.unseen = 0;
    }
}

pub fn draw(frame: &mut Frame, model: &Model, view: &mut View, config: &Config) {
    let palette = config.theme.palette();
    let light = matches!(config.theme, crate::Theme::Light);
    let duration = std::time::Duration::from_millis(config.animation_ms);
    let bg = Color::Reset;
    let fg = palette.foreground;
    let muted = palette.muted;
    let accent = palette.accent;
    let times: Vec<_> = model.modification_times.keys().copied().collect();
    frame.render_widget(
        Block::default().style(Style::default().bg(bg).fg(fg)),
        frame.area(),
    );
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(frame.area());
    let state = if model.refreshing {
        "REFRESHING"
    } else if model.watch_status.is_some() {
        "RECOVERY ONLY"
    } else {
        "WATCHING"
    };
    let phase = view
        .started
        .map_or(0, |start| start.elapsed().as_millis() / 80);
    let indicator = if model.refreshing && !config.no_animations && !config.reduced_motion {
        ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"][phase as usize % 10]
    } else {
        "●"
    };
    let mut counts = [0usize; 4];
    for file in model.files.values() {
        counts[match file.status {
            Status::New => 0,
            Status::Modified => 1,
            Status::Deleted => 2,
            Status::Unchanged => 3,
        }] += 1;
    }
    let positive = palette.positive;
    let negative = palette.negative;
    let gold = palette.gold;
    let compact = frame.area().width < 72;
    let comparison = match (model.baseline.as_str(), compact) {
        ("last clear", true) => "vs last clear",
        ("last clear", false) => "Comparing: last clear",
        (_, true) => "vs HEAD",
        (_, false) => "Comparing: HEAD",
    };
    let header = Line::from(vec![
        Span::styled(
            " ◈ RECON │ ",
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            comparison,
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" │ {indicator} {state}"),
            Style::default().fg(if model.refreshing { gold } else { positive }),
        ),
        Span::styled(format!(" │ {}", model.branch), Style::default().fg(muted)),
    ]);
    let legend = age_legend(&palette, false);
    // Measure the actual branch/status text before reserving the legend's space.
    // Hide it entirely rather than clipping either the header or the example.
    let legend_width = if header.width() + 2 + legend.width() <= parts[0].width as usize {
        legend.width() as u16
    } else {
        0
    };
    let header_cols =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(legend_width)]).split(parts[0]);
    frame.render_widget(Paragraph::new(header), header_cols[0]);
    if legend_width > 0 {
        frame.render_widget(Paragraph::new(legend), header_cols[1]);
    }
    let narrow = parts[1].width < 72;
    let tree_visible = !view.sidebar || !narrow || !view.panel_focus;
    let panel_visible = view.sidebar && (!narrow || view.panel_focus);
    view.split_area = parts[1];
    view.layout_x = parts[1].x;
    view.layout_width = parts[1].width;
    view.divider = None;
    let (tree_rect, panel_rect) = if tree_visible && panel_visible {
        let width = ((parts[1].width as u32 * view.tree_percent as u32) / 100) as u16;
        let width = width.clamp(18, parts[1].width.saturating_sub(24));
        let cols =
            Layout::horizontal([Constraint::Length(width), Constraint::Min(24)]).split(parts[1]);
        view.divider = Some(cols[1].x);
        (cols[0], cols[1])
    } else {
        (parts[1], parts[1])
    };
    if tree_visible {
        let rows = tree(model, view);
        let selected = view
            .selected
            .as_ref()
            .and_then(|p| rows.iter().position(|r| &r.key == p));
        let items: Vec<_> = rows
            .iter()
            .map(|r| {
                let indent = "│ ".repeat(r.depth);
                let name = r
                    .key
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or(&r.key);
                if r.dir {
                    ListItem::new(Line::from(vec![Span::styled(
                        format!(
                            "      {indent}{} {name}/",
                            if view.search.is_empty() && view.collapsed.contains(&r.key) {
                                "▸"
                            } else {
                                "▾"
                            }
                        ),
                        Style::default().fg(muted),
                    )]))
                } else {
                    let file = &model.files[&r.key];
                    let color = match file.status {
                        Status::New => positive,
                        Status::Modified => gold,
                        Status::Deleted | Status::Unchanged => muted,
                    };
                    let age = file.activity.elapsed();
                    let motion_age = view
                        .arrivals
                        .get(&file.path)
                        .map_or(duration, Instant::elapsed);
                    let distance = tree_rect.width.saturating_sub(12 + (r.depth as u16) * 2);
                    let motion = animation::slide(
                        motion_age,
                        duration,
                        !config.no_animations && !config.reduced_motion,
                        false,
                        distance,
                    );
                    let deleted = file.status == Status::Deleted;
                    let motion = if deleted { 0 } else { motion };
                    let active = age < duration && !config.no_animations && !deleted;
                    let color = animation::glow(
                        color,
                        age,
                        duration,
                        !config.no_animations && !config.reduced_motion && !deleted,
                        light,
                    );
                    let git_status: String = file.git.chars().take(2).collect();
                    let git_color = if git_status.contains('D') {
                        negative
                    } else if git_status == "??" || git_status.contains('A') {
                        positive
                    } else if git_status.trim().is_empty() {
                        muted
                    } else {
                        gold
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(format!(" {git_status:2} "), Style::default().fg(git_color)),
                        Span::styled(
                            "● ",
                            Style::default()
                                .fg(palette
                                    .recency(animation::relative_age(file.modified_at, &times))),
                        ),
                        Span::styled(
                            format!(
                                "{indent}{}{} {name}",
                                " ".repeat((2 + motion).max(0) as usize),
                                file.status.symbol()
                            ),
                            Style::default().fg(color).add_modifier(if active {
                                Modifier::BOLD
                            } else {
                                Modifier::empty()
                            }),
                        ),
                        Span::styled(
                            file.git.get(2..).unwrap_or("").to_owned(),
                            Style::default().fg(muted),
                        ),
                    ]))
                }
            })
            .collect();
        let mut stats = vec![
            Span::styled(format!(" +{} ", counts[0]), Style::default().fg(positive)),
            Span::styled(format!("~{} ", counts[1]), Style::default().fg(gold)),
            Span::styled(format!("−{} ", counts[2]), Style::default().fg(negative)),
            Span::styled(format!("·{}", counts[3]), Style::default().fg(muted)),
        ];
        if tree_rect.width >= 85 {
            stats.push(Span::styled(
                format!(
                    " │ {} entries · {:.1} MiB",
                    model.entries.len(),
                    model.bytes as f64 / 1048576.
                ),
                Style::default().fg(muted),
            ));
        }
        let stats_width = Line::from(stats.clone()).width();
        let available = usize::from(tree_rect.width.saturating_sub(2));
        let root_label = " │ Root: ";
        let root = tail_path(
            &model.root.display().to_string(),
            available.saturating_sub(stats_width + Line::raw(root_label).width() + 1),
        );
        stats.push(Span::styled(
            format!("{root_label}{root} "),
            Style::default().fg(muted),
        ));
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Files ")
            .title_bottom(Line::from(stats))
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(if !view.panel_focus { accent } else { muted }));
        let list = List::new(items)
            .highlight_symbol("▸ ")
            .highlight_spacing(HighlightSpacing::Always)
            .block(block)
            .highlight_style(Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED));
        let mut state = ListState::default()
            .with_offset(view.tree_offset)
            .with_selected(selected);
        frame.render_stateful_widget(list, tree_rect, &mut state);
        view.tree_offset = state.offset();
        // Keep the muted tombstone in place while a coloured copy departs over it.
        // Render only the label so Git status, recency, and tree guides stay fixed.
        if !config.no_animations && !config.reduced_motion {
            let inner = inset(tree_rect);
            for (index, row) in rows
                .iter()
                .skip(state.offset())
                .take(inner.height as usize)
                .enumerate()
                .filter(|(_, row)| !row.dir)
            {
                let file = &model.files[&row.key];
                let age = file.activity.elapsed();
                if file.status != Status::Deleted || age >= duration {
                    continue;
                }
                let distance = tree_rect.width.saturating_sub(12 + row.depth as u16 * 2);
                // A short flight leaves room to watch the copy dissolve even
                // when the tree pane is narrow.
                let motion = animation::slide(age, duration, true, true, distance.min(16)) as u16;
                // List reserves two cells for the selection marker, then six for
                // Git status/recency, followed by indentation and label padding.
                let x = inner.x.saturating_add(10 + row.depth as u16 * 2 + motion);
                let right = inner.right();
                if x >= right {
                    continue;
                }
                let name = row.key.rsplit('/').next().unwrap_or(&row.key);
                let color = animation::glow(negative, age, duration, true, light);
                let mut style = Style::default().fg(color).add_modifier(Modifier::BOLD);
                if selected == Some(state.offset() + index) {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                render_departing_label(
                    &format!("− {name}"),
                    style,
                    animation::progress(age, duration),
                    Rect::new(x, inner.y + index as u16, right - x, 1),
                    frame.buffer_mut(),
                );
            }
        }
        if rows.is_empty() {
            frame.render_widget(
                Paragraph::new(if view.search.is_empty() {
                    " No changes yet.\n Edits will appear here."
                } else {
                    " No matching files.\n Esc clears the search."
                })
                .style(Style::default().fg(muted)),
                inset(tree_rect),
            );
        }
    }
    if panel_visible {
        let label = view.filter.as_deref().unwrap_or(if view.search.is_empty() {
            "All files"
        } else {
            "Matching files"
        });
        let paused = format!(
            "PAUSED • {} to resume",
            config.keybinds.label(Action::Follow)
        );
        let title = format!(
            " History • {label} • {} {} ",
            if view.following { "LIVE" } else { &paused },
            if view.unseen > 0 {
                format!("• {} new", view.unseen)
            } else {
                String::new()
            }
        );
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(if view.panel_focus || view.dragging {
                accent
            } else {
                muted
            }));
        let inner = block.inner(panel_rect);
        frame.render_widget(block, panel_rect);
        let height = inner.height as usize;
        let count = total(model, view);
        if view.following {
            view.scroll = 0;
        }
        view.scroll = view.scroll.min(count.saturating_sub(height));
        let mut lines = Vec::new();
        let mut row = 0;
        for entry in model
            .entries
            .iter()
            .rev()
            .filter(|e| entry_matches(e, view))
        {
            let n = length(entry);
            if row + n <= view.scroll {
                row += n;
                continue;
            }
            if row >= view.scroll + height {
                break;
            }
            let secs = entry
                .time
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                % 86400;
            let time = format!(
                "{:02}:{:02}:{:02}Z",
                secs / 3600,
                secs % 3600 / 60,
                secs % 60
            );
            let age = entry.time.elapsed().unwrap_or(duration);
            let shift = " ".repeat(
                (2 + animation::slide(
                    age,
                    duration,
                    !config.no_animations && !config.reduced_motion,
                    false,
                    inner.width.saturating_sub(4),
                ))
                .max(0) as usize,
            );
            let header_color = animation::glow(
                accent,
                age,
                duration,
                !config.no_animations && !config.reduced_motion,
                light,
            );
            let header = format!(
                "{shift}╭─ {} {}  {}  • {}",
                if entry.collapsed { "▸" } else { "▾" },
                entry.path,
                time,
                entry.activity
            );
            let mut local = Vec::new();
            local.push(Line::styled(
                header,
                Style::default()
                    .fg(header_color)
                    .add_modifier(Modifier::BOLD),
            ));
            if entry.collapsed {
                local.push(Line::styled(
                    format!("{shift}╰─ {} lines • Enter to expand", entry.line_count),
                    Style::default().fg(muted),
                ));
            } else {
                // Only materialize visible lines, even for very large retained entries.
                let from = view.scroll.saturating_sub(row).saturating_sub(1);
                for (i, line) in entry.diff.lines().enumerate().skip(from).take(height + 1) {
                    let color = if line.starts_with('+') {
                        positive
                    } else if line.starts_with('-') {
                        negative
                    } else if line.starts_with("@@") {
                        accent
                    } else {
                        fg
                    };
                    // Put lines directly by their virtual row (no allocation for offscreen text).
                    let absolute = row + i + 1;
                    if absolute >= view.scroll && absolute < view.scroll + height {
                        lines.push((
                            absolute - view.scroll,
                            Line::from(vec![
                                Span::raw(shift.clone()),
                                Span::styled("│ ", Style::default().fg(muted)),
                                Span::styled(line.to_owned(), Style::default().fg(color)),
                            ]),
                        ));
                    }
                }
            }
            for (i, line) in local.into_iter().enumerate() {
                let absolute = row + i;
                if absolute >= view.scroll && absolute < view.scroll + height {
                    lines.push((absolute - view.scroll, line));
                }
            }
            if !entry.collapsed {
                let bottom = row + n - 1;
                if bottom >= view.scroll && bottom < view.scroll + height {
                    lines.push((
                        bottom - view.scroll,
                        Line::styled(
                            format!("{shift}╰────────────────"),
                            Style::default().fg(muted),
                        ),
                    ));
                }
            }
            row += n;
        }
        for (y, line) in lines {
            frame.render_widget(
                Paragraph::new(line),
                Rect::new(inner.x, inner.y + y as u16, inner.width, 1),
            );
        }
        if count == 0 {
            frame.render_widget(
                Paragraph::new(format!(
                    " Waiting for observed changes…\n {} clear   {} HEAD",
                    config.keybinds.label(Action::Clear),
                    config.keybinds.label(Action::GitSnapshot)
                ))
                .style(Style::default().fg(muted)),
                inner,
            );
        }
    }
    let footer = [
        (Action::Help, "help"),
        (Action::Search, "search"),
        (Action::Clear, "clear"),
        (Action::GitSnapshot, "HEAD"),
        (Action::ToggleHistory, "panel"),
        (Action::Focus, "focus"),
        (Action::Quit, "quit"),
    ]
    .into_iter()
    .map(|(a, text)| format!("{} {text}", config.keybinds.label(a)))
    .collect::<Vec<_>>()
    .join("  ");
    let status = format!(
        " {}{}{}",
        if model.refreshing {
            &model.message
        } else {
            &model.notice
        },
        if model.evicted > 0 {
            format!(" • {} oldest entries evicted", model.evicted)
        } else {
            String::new()
        },
        if !view.sidebar && view.unseen > 0 {
            format!(" • {} updates while hidden", view.unseen)
        } else {
            String::new()
        }
    );
    let searching = view.searching || !view.search.is_empty();
    let show_status = !status.trim().is_empty();
    let footer_line = if searching {
        Line::from(vec![
            Span::styled(
                format!(" /{}{}", view.search, if view.searching { "▏" } else { "" }),
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if view.searching {
                    "  Enter keep · Esc cancel · Ctrl-U clear".to_string()
                } else {
                    format!(
                        "  {}/{} next/previous · {} clear",
                        config.keybinds.label(Action::NextMatch),
                        config.keybinds.label(Action::PreviousMatch),
                        config.keybinds.label(Action::AllHistory)
                    )
                },
                Style::default().fg(muted),
            ),
        ])
    } else if show_status {
        if !config.no_animations && !config.reduced_motion {
            if let Some(remaining) = model.notice_remaining(Instant::now()) {
                notification_line(&status, remaining, accent, muted)
            } else {
                Line::styled(status, Style::default().fg(accent))
            }
        } else {
            Line::styled(status, Style::default().fg(accent))
        }
    } else {
        Line::styled(footer, Style::default().fg(muted))
    };
    if searching || show_status {
        let shortcuts = format!(
            " {} help  {} quit",
            config.keybinds.label(Action::Help),
            config.keybinds.label(Action::Quit)
        );
        let columns = Layout::horizontal([
            Constraint::Min(0),
            Constraint::Length(shortcuts.chars().count() as u16),
        ])
        .split(parts[2]);
        frame.render_widget(Paragraph::new(footer_line), columns[0]);
        frame.render_widget(
            Paragraph::new(shortcuts).style(Style::default().fg(muted)),
            columns[1],
        );
    } else {
        frame.render_widget(Paragraph::new(footer_line), parts[2]);
    }
    if view.help {
        let area = frame.area();
        let popup = Rect::new(
            area.width.saturating_sub(66) / 2,
            area.height.saturating_sub(21) / 2,
            area.width.min(66),
            area.height.min(21),
        );
        frame.render_widget(Clear, popup);
        let groups: &[(&[Action], &str)] = &[
            (&[Action::Search], "Search file paths"),
            (
                &[Action::Clear],
                "Clear history; compare from current contents",
            ),
            (
                &[Action::GitSnapshot],
                "Clear history; compare against HEAD",
            ),
            (&[Action::ToggleHistory], "Hide/show history"),
            (&[Action::Focus], "Switch focus / narrow-terminal view"),
            (&[Action::Down, Action::Up], "Navigate / scroll down/up"),
            (
                &[Action::Collapse, Action::Expand],
                "Collapse/expand directory",
            ),
            (
                &[Action::WidenHistory, Action::NarrowHistory],
                "Widen/narrow history",
            ),
            (&[Action::Open], "Open file history / toggle fold"),
            (
                &[Action::NextMatch, Action::PreviousMatch],
                "Next/previous search result",
            ),
            (&[Action::PageDown, Action::PageUp], "Page down/up"),
            (
                &[Action::HalfPageDown, Action::HalfPageUp],
                "Half-page down/up",
            ),
            (&[Action::Follow], "Toggle live following"),
            (&[Action::AllHistory], "Clear search; all-file history"),
            (
                &[Action::Help, Action::Quit],
                "Help (any key closes) / quit",
            ),
        ];
        let mut lines: Vec<Line> = groups
            .iter()
            .map(|(actions, description)| {
                let keys = actions
                    .iter()
                    .map(|&a| config.keybinds.label(a))
                    .collect::<Vec<_>>()
                    .join(" / ");
                Line::from(format!(" {keys}  {description}"))
            })
            .collect();
        lines.push(Line::from(
            " Search input: Enter keep · Esc cancel · Ctrl-U clear",
        ));
        lines.push(age_legend(&palette, false));
        lines.push(Line::from(" Age ranks file mtimes; not elapsed time."));
        lines.push(Line::from(
            " Git: index/worktree; ?? untracked. Drag divider to resize.",
        ));
        frame.render_widget(
            Paragraph::new(lines)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .title(" Recon • keyboard "),
                )
                .style(Style::default().bg(bg).fg(fg)),
            popup,
        );
    }
}
/// The accent-coloured portion drains from right to left with a soft edge;
/// expired characters remain readable in the muted colour until dismissal.
fn notification_line(message: &str, remaining: f64, accent: Color, muted: Color) -> Line<'static> {
    let span = Span::raw(message);
    let width = span.width().max(1) as f64;
    let mut column = 0;
    let spans = span.styled_graphemes(Style::default()).map(|grapheme| {
        let cells = Span::raw(grapheme.symbol).width();
        let position = (column as f64 + cells as f64 / 2.0) / width;
        column += cells;
        let fade = ((position - remaining) / 0.15 + 1.0 - remaining).clamp(0.0, 1.0);
        Span::styled(
            grapheme.symbol.to_owned(),
            Style::default().fg(animation::gradient(fade, [accent, accent, muted, muted])),
        )
    });
    Line::from(spans.collect::<Vec<_>>())
}

fn age_legend(palette: &crate::theme::Palette, compact: bool) -> Line<'static> {
    let mut spans = vec![Span::styled(
        if compact {
            " Age: new "
        } else {
            " Age: newest "
        },
        Style::default().fg(palette.muted),
    )];
    let steps = if compact { 4 } else { 12 };
    spans.extend((0..=steps).map(|step| {
        Span::styled(
            "●",
            Style::default().fg(palette.recency(step as f64 / steps as f64)),
        )
    }));
    spans.push(Span::styled(
        if compact {
            " old "
        } else {
            " oldest (relative mtime) "
        },
        Style::default().fg(palette.muted),
    ));
    Line::from(spans)
}

/// Disappearing glyphs are transparent: they reveal the tombstone rather
/// than painting spaces over it. Rasterizing first preserves wide/combining text.
fn render_departing_label(
    label: &str,
    style: Style,
    progress: f64,
    area: Rect,
    buffer: &mut Buffer,
) {
    let mut copy = Buffer::empty(area);
    Line::from(Span::styled(label, style)).render(area, &mut copy);
    let dissolve = ((progress - 0.45) / 0.5).clamp(0.0, 1.0);
    let mut x = area.x;
    while x < area.right() {
        let cell = &copy[(x, area.y)];
        let width = Span::raw(cell.symbol()).width().max(1) as u16;
        // Stable, scattered thresholds keep the effect smooth between frames
        // and avoid making an entire filename disappear in one step.
        let threshold = ((usize::from(x - area.x) * 37 + 17) % 101) as f64 / 100.0;
        if cell.symbol() != " " && (dissolve == 0.0 || dissolve < threshold) {
            for dx in 0..width.min(area.right() - x) {
                buffer[(x + dx, area.y)] = copy[(x + dx, area.y)].clone();
            }
        }
        x = x.saturating_add(width);
    }
}

/// Shorten from the left without splitting terminal grapheme clusters.
fn tail_path(path: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let span = Span::raw(path);
    if span.width() <= width {
        return path.to_owned();
    }
    let graphemes: Vec<_> = span.styled_graphemes(Style::default()).collect();
    let mut used = 1; // ellipsis
    let mut start = graphemes.len();
    for (index, grapheme) in graphemes.iter().enumerate().rev() {
        let next = Span::raw(grapheme.symbol).width();
        if used + next > width {
            break;
        }
        used += next;
        start = index;
    }
    let mut result = String::from("…");
    for grapheme in &graphemes[start..] {
        result.push_str(grapheme.symbol);
    }
    result
}

fn inset(r: Rect) -> Rect {
    Rect::new(
        r.x.saturating_add(1),
        r.y.saturating_add(1),
        r.width.saturating_sub(2),
        r.height.saturating_sub(2),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use ratatui::backend::TestBackend;
    #[test]
    fn notification_gradient_drains_without_changing_the_message_or_background() {
        let message = "Cleared • observe 界e\u{301} changes";
        let accent = Color::Rgb(200, 100, 50);
        let muted = Color::Rgb(100, 100, 100);
        for remaining in [1.0, 0.5, 0.0] {
            let line = notification_line(message, remaining, accent, muted);
            let text: String = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            assert_eq!(text, message);
            assert!(line.spans.iter().all(|span| span.style.bg.is_none()));
            if remaining == 1.0 {
                assert!(line.spans.iter().all(|span| span.style.fg == Some(accent)));
            } else if remaining == 0.0 {
                assert!(line.spans.iter().all(|span| span.style.fg == Some(muted)));
            } else {
                assert_eq!(line.spans.first().unwrap().style.fg, Some(accent));
                assert_eq!(line.spans.last().unwrap().style.fg, Some(muted));
                assert!(
                    line.spans
                        .iter()
                        .any(|span| span.style.fg != Some(accent) && span.style.fg != Some(muted))
                );
            }
        }
    }

    #[test]
    fn expired_or_dismissed_notifications_restore_the_full_keybind_footer() {
        let config = Config::parse_from(["recon", "--no-animations"]);
        let mut model = Model::new(10, 1024);
        model.apply(crate::model::Update::Done);
        let mut view = View::new();
        let mut terminal = Terminal::new(TestBackend::new(140, 12)).unwrap();
        let footer = |terminal: &Terminal<TestBackend>| -> String {
            terminal
                .backend()
                .buffer()
                .content
                .chunks(140)
                .last()
                .unwrap()
                .iter()
                .map(|cell| cell.symbol())
                .collect()
        };
        for expire in [true, false] {
            model.apply(crate::model::Update::Notice(
                "Cleared demo notification".into(),
            ));
            terminal
                .draw(|frame| draw(frame, &model, &mut view, &config))
                .unwrap();
            assert!(footer(&terminal).contains("Cleared demo notification"));
            if expire {
                assert!(model.expire_notice(Instant::now() + std::time::Duration::from_secs(4)));
            } else {
                assert!(model.dismiss_notice());
            }
            terminal
                .draw(|frame| draw(frame, &model, &mut view, &config))
                .unwrap();
            let text = footer(&terminal);
            assert!(text.contains("c clear"));
            assert!(text.contains("g HEAD"));
            assert!(!text.contains("Cleared demo notification"));
        }
    }

    #[test]
    fn layouts_render_at_tiny_and_wide_sizes() {
        let config = Config::parse_from(["recon"]);
        for (w, h) in [(1, 1), (30, 10), (80, 24), (140, 40)] {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            let m = Model::new(100, 4096);
            let mut v = View::new();
            for panel in [false, true] {
                v.panel_focus = panel;
                term.draw(|f| draw(f, &m, &mut v, &config)).unwrap();
            }
        }
    }
    #[test]
    fn files_bottom_border_shows_counts_and_root_without_using_a_content_row() {
        let config = Config::parse_from(["recon", "--no-animations"]);
        let mut model = Model::new(100, 4096);
        model.root = "/Users/oli/Development/long-parent-directory/界/recon".into();
        for width in [30, 160] {
            let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
            let mut view = View::new();
            view.sidebar = false;
            terminal
                .draw(|frame| draw(frame, &model, &mut view, &config))
                .unwrap();
            let line: String = terminal
                .backend()
                .buffer()
                .content
                .chunks(width as usize)
                .nth(10)
                .unwrap()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(line.contains("+0"));
            assert!(line.contains("~0"));
            assert!(line.contains("Root: "));
            assert!(line.contains("recon"));
            assert_eq!(view.split_area.y, 1);
            assert_eq!(view.split_area.height, 10);
            if width == 160 {
                assert!(line.contains("/Users/oli/Development/long-parent-directory/"));
                assert!(line.contains('界'));
                assert!(!line.contains('…'));
            } else {
                assert!(line.contains('…'));
                assert!(!line.contains("/Users/oli"));
            }
        }
    }

    #[test]
    fn themes_show_matching_age_legend_baseline_and_configured_hints() {
        for theme in ["dark", "light", "ashen"] {
            let mut config = Config::parse_from(["recon", "--theme", theme, "--no-animations"]);
            config.keybinds = crate::keybinds::Keybinds::with_overrides(BTreeMap::from([
                (Action::Clear, vec!["Alt-c".into()]),
                (Action::Quit, vec!["Ctrl-x".into()]),
            ]))
            .unwrap();
            let mut model = Model::new(100, 4096);
            let mut view = View::new();
            let mut terminal = Terminal::new(TestBackend::new(160, 24)).unwrap();
            for baseline in ["HEAD", "last clear"] {
                model.baseline = baseline.into();
                terminal
                    .draw(|f| draw(f, &model, &mut view, &config))
                    .unwrap();
                let buffer = terminal.backend().buffer();
                let text: String = buffer.content.iter().map(|c| c.symbol()).collect();
                assert!(text.contains(if baseline == "HEAD" {
                    "Comparing: HEAD"
                } else {
                    "Comparing: last clear"
                }));
                assert!(!text.contains("FPS"));
                assert!(text.contains("Alt-c clear"));
                assert!(text.contains("Ctrl-x quit"));
                let dots: Vec<_> = buffer
                    .content
                    .chunks(160)
                    .next()
                    .unwrap()
                    .iter()
                    .skip(160 - age_legend(&config.theme.palette(), false).width())
                    .filter(|c| c.symbol() == "●")
                    .collect();
                assert_eq!(dots.len(), 13);
                for (step, dot) in dots.iter().enumerate() {
                    assert_eq!(dot.fg, config.theme.palette().recency(step as f64 / 12.));
                }
            }
            view.help = true;
            terminal
                .draw(|f| draw(f, &model, &mut view, &config))
                .unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect();
            assert!(text.contains("Alt-c  Clear history"));
            assert!(text.contains("Ctrl-x  Help")); // grouped Help / Quit label
        }
    }
    #[test]
    fn top_right_age_legend_hides_when_it_would_crowd_the_header() {
        let config = Config::parse_from(["recon", "--no-animations"]);
        let mut model = Model::new(100, 4096);
        model.refreshing = false;
        let long_branch = format!("feature/{}", "x".repeat(80));
        for (width, branch, visible) in [
            (80, "main", false),
            (160, "main", true),
            (160, long_branch.as_str(), false),
            (240, long_branch.as_str(), true),
        ] {
            model.branch = branch.into();
            let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
            let mut view = View::new();
            terminal
                .draw(|frame| draw(frame, &model, &mut view, &config))
                .unwrap();
            let row: String = terminal.backend().buffer().content[..width as usize]
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(row.contains(branch));
            assert_eq!(row.contains("Age:"), visible);
            if visible {
                assert!(row.ends_with("oldest (relative mtime) "));
            }
            assert_eq!(view.split_area.y, 1);
        }
    }

    #[test]
    fn compact_chrome_keeps_content_and_status_visible_in_short_terminals() {
        let config = Config::parse_from(["recon", "--no-animations"]);
        let mut model = history_model();
        model.refreshing = false;
        model.baseline = "last clear".into();
        model.apply(crate::model::Update::WatchStatus(
            "Watcher unavailable; recovering".into(),
        ));
        for (width, height) in [(36, 12), (100, 12), (100, 24)] {
            let mut view = View::new();
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|f| draw(f, &model, &mut view, &config))
                .unwrap();
            let rows: Vec<String> = terminal
                .backend()
                .buffer()
                .content
                .chunks(width as usize)
                .map(|row| row.iter().map(|c| c.symbol()).collect())
                .collect();
            assert_eq!(view.split_area.y, 1);
            assert_eq!(view.split_area.height, height - 2);
            assert!(rows[0].contains("last clear"));
            assert!(!rows[0].contains("Age:"));
            assert!(rows[3].contains("file0.rs"));
            assert!(rows[height as usize - 1].contains("Watcher"));
            assert!(rows[height as usize - 1].contains("q quit"));
            view.searching = true;
            view.search = "file".into();
            terminal
                .draw(|f| draw(f, &model, &mut view, &config))
                .unwrap();
            let footer: String = terminal
                .backend()
                .buffer()
                .content
                .chunks(width as usize)
                .last()
                .unwrap()
                .iter()
                .map(|c| c.symbol())
                .collect();
            assert!(footer.contains("/file"));
            assert!(footer.contains("q quit"));
            assert_eq!(view.split_area.height, height - 2);
        }
    }
    fn history_model() -> Model {
        let mut model = Model::new(100, 1_000_000);
        let mut rows = Vec::new();
        for id in 0..8 {
            let path = format!("dir/file{id}.rs");
            rows.push((
                crate::model::FileRow {
                    path: path.clone(),
                    status: Status::Modified,
                    git: " M".into(),
                    note: String::new(),
                    activity: Instant::now(),
                    remove_from_tree: false,
                    modified_at: None,
                },
                Some(crate::model::Entry {
                    id,
                    path,
                    time: std::time::SystemTime::now(),
                    activity: "edited".into(),
                    diff: "-before\n+after".into(),
                    collapsed: false,
                    line_count: 2,
                }),
            ));
        }
        model.apply(crate::model::Update::Files(rows));
        model
    }
    #[test]
    fn sidebar_preserves_filter_scroll_and_focus_rules() {
        let mut m = history_model();
        let mut v = View::new();
        let (tx, _rx) = std::sync::mpsc::sync_channel(8);
        v.filter = Some("dir/file0.rs".into());
        v.scroll = 2;
        v.following = false;
        v.panel_focus = true;
        handle_key(KeyCode::Char('d').into(), &mut m, &mut v, &tx, 24);
        assert!(!v.sidebar);
        assert!(!v.panel_focus);
        handle_key(KeyCode::Down.into(), &mut m, &mut v, &tx, 24);
        assert!(!v.sidebar); // Navigating a hidden sidebar never reopens it.
        v.filter = Some("dir/file0.rs".into());
        v.scroll = 2;
        v.following = false;
        handle_key(KeyCode::Char('d').into(), &mut m, &mut v, &tx, 24);
        assert!(v.sidebar);
        assert_eq!(v.scroll, 2);
        assert_eq!(v.filter.as_deref(), Some("dir/file0.rs"));
    }
    #[test]
    fn reading_anchor_survives_eviction_and_tree_identity_survives_insertion() {
        let mut m = history_model();
        let mut v = View::new();
        v.scroll = 12;
        v.following = false;
        v.selected = Some("dir/file5.rs".into());
        save_anchor(&m, &mut v);
        assert_eq!(v.anchor, Some((4, 0)));
        m.entries.pop_front();
        restore_anchor(&m, &mut v);
        assert_eq!(v.scroll, 12);
        let mut row = m.files["dir/file5.rs"].clone();
        row.path = "aaa".into();
        m.files.insert(row.path.clone(), row);
        assert!(
            tree(&m, &v)
                .iter()
                .any(|r| Some(&r.key) == v.selected.as_ref())
        );
        let (tx, _rx) = std::sync::mpsc::sync_channel(8);
        handle_key(KeyCode::Left.into(), &mut m, &mut v, &tx, 24);
        assert!(v.collapsed.contains("dir/"));
        assert_eq!(v.selected.as_deref(), Some("dir/"));
        handle_key(KeyCode::Right.into(), &mut m, &mut v, &tx, 24);
        assert!(!v.collapsed.contains("dir/"));
    }
    #[test]
    fn search_matches_collapsed_paths_and_supports_accept_cancel_and_next() {
        let mut model = history_model();
        let mut view = View::new();
        let (tx, _rx) = std::sync::mpsc::sync_channel(8);
        view.collapsed.insert("dir/".into());
        view.sidebar = false;
        handle_key(KeyCode::Char('/').into(), &mut model, &mut view, &tx, 24);
        for c in "FILE3".chars() {
            handle_key(KeyCode::Char(c).into(), &mut model, &mut view, &tx, 24);
        }
        let matches: Vec<_> = tree(&model, &view).into_iter().filter(|r| !r.dir).collect();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].key, "dir/file3.rs");
        assert!(!view.sidebar);
        assert_eq!(view.filter.as_deref(), Some("dir/file3.rs"));
        handle_key(KeyCode::Enter.into(), &mut model, &mut view, &tx, 24);
        assert!(!view.searching);
        handle_key(KeyCode::Char('/').into(), &mut model, &mut view, &tx, 24);
        handle_key(KeyCode::Char('x').into(), &mut model, &mut view, &tx, 24);
        assert!(tree(&model, &view).is_empty());
        assert_eq!(total(&model, &view), 0);
        handle_key(KeyCode::Esc.into(), &mut model, &mut view, &tx, 24);
        assert_eq!(view.search, "FILE3");
        assert_eq!(view.filter.as_deref(), Some("dir/file3.rs"));
        handle_key(KeyCode::Esc.into(), &mut model, &mut view, &tx, 24);
        assert!(view.search.is_empty());
        assert!(view.filter.is_none());
        view.collapsed.clear();
        view.search = "file".into();
        search_selection(&model, &mut view);
        handle_key(KeyCode::Char('n').into(), &mut model, &mut view, &tx, 24);
        assert_eq!(view.selected.as_deref(), Some("dir/file4.rs"));
        handle_key(KeyCode::Char('N').into(), &mut model, &mut view, &tx, 24);
        assert_eq!(view.selected.as_deref(), Some("dir/file3.rs"));
    }
    #[test]
    fn divider_drag_and_keys_preserve_width_and_bound_both_panes() {
        let mut model = history_model();
        let mut view = View::new();
        let config = Config::parse_from(["recon"]);
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        terminal
            .draw(|f| draw(f, &model, &mut view, &config))
            .unwrap();
        let divider = view.divider.unwrap();
        let mouse = |kind, x| MouseEvent {
            kind,
            column: x,
            row: 5,
            modifiers: KeyModifiers::NONE,
        };
        assert!(handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), divider),
            &mut view
        ));
        assert!(handle_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Left), 72),
            &mut view
        ));
        assert_eq!(view.tree_percent, 60);
        handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 72), &mut view);
        let (tx, _rx) = std::sync::mpsc::sync_channel(8);
        handle_key(KeyCode::Char('[').into(), &mut model, &mut view, &tx, 24);
        assert_eq!(view.tree_percent, 55);
        handle_key(KeyCode::Char('d').into(), &mut model, &mut view, &tx, 24);
        terminal
            .draw(|f| draw(f, &model, &mut view, &config))
            .unwrap();
        assert!(view.divider.is_none());
        handle_key(KeyCode::Char('d').into(), &mut model, &mut view, &tx, 24);
        assert_eq!(view.tree_percent, 55);
        view.tree_percent = 90;
        let mut narrow = Terminal::new(TestBackend::new(72, 24)).unwrap();
        narrow
            .draw(|f| draw(f, &model, &mut view, &config))
            .unwrap();
        assert!(view.divider.unwrap() <= 48);
        assert_eq!(view.tree_percent, 90);
        let mut tiny = Terminal::new(TestBackend::new(30, 10)).unwrap();
        tiny.draw(|f| draw(f, &model, &mut view, &config)).unwrap();
        assert!(view.divider.is_none());
    }
    #[test]
    fn status_column_stays_aligned_during_motion_at_different_depths() {
        let mut model = history_model();
        let mut view = View::new();
        let mut root = model.files["dir/file0.rs"].clone();
        root.path = "rootfile.rs".into();
        model.files.insert(root.path.clone(), root);
        let mut terminal = Terminal::new(TestBackend::new(140, 24)).unwrap();
        let config = Config::parse_from(["recon"]);
        terminal
            .draw(|f| draw(f, &model, &mut view, &config))
            .unwrap();
        let width = view.divider.unwrap() as usize;
        let columns: Vec<_> = terminal
            .backend()
            .buffer()
            .content
            .chunks(140)
            .map(|row| {
                row.iter()
                    .take(width)
                    .map(|c| c.symbol())
                    .collect::<String>()
            })
            .filter(|row| row.contains("file") && row.contains('M'))
            .map(|row| row.chars().position(|c| c == 'M').unwrap())
            .collect();
        assert!(columns.len() >= 2);
        assert!(columns.iter().all(|x| *x == columns[0]));
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .all(|cell| cell.bg == Color::Reset)
        );
    }
    #[test]
    fn enter_on_file_opens_and_focuses_sidebar_without_collapsing_parent() {
        let mut model = history_model();
        let mut view = View::new();
        let (tx, _rx) = std::sync::mpsc::sync_channel(8);
        view.selected = Some("dir/file3.rs".into());
        view.filter = view.selected.clone();
        view.sidebar = false;
        view.scroll = 2;
        view.following = false;
        handle_key(KeyCode::Enter.into(), &mut model, &mut view, &tx, 24);
        assert!(view.sidebar && view.panel_focus);
        assert!(!view.collapsed.contains("dir/"));
        assert_eq!(view.selected.as_deref(), Some("dir/file3.rs"));
        assert_eq!(view.scroll, 2);
        assert!(!view.following);
        view.panel_focus = false;
        view.selected = Some("dir/".into());
        handle_key(KeyCode::Enter.into(), &mut model, &mut view, &tx, 24);
        assert!(view.collapsed.contains("dir/"));
    }
    #[test]
    fn control_scrolls_half_pages_and_f_toggles_live_without_hiding_sidebar() {
        let mut model = history_model();
        let mut view = View::new();
        let (tx, _rx) = std::sync::mpsc::sync_channel(8);
        view.panel_focus = true;
        view.scroll = 14;
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        handle_key(ctrl('u'), &mut model, &mut view, &tx, 24);
        assert_eq!(view.scroll, 4);
        assert!(!view.following);
        assert!(view.sidebar);
        handle_key(ctrl('d'), &mut model, &mut view, &tx, 24);
        assert_eq!(view.scroll, 13); // clamp to the last full page
        assert!(view.sidebar);
        handle_key(KeyCode::Char('f').into(), &mut model, &mut view, &tx, 24);
        assert!(view.following);
        handle_key(KeyCode::Char('f').into(), &mut model, &mut view, &tx, 24);
        assert!(!view.following);
        view.panel_focus = false;
        handle_key(ctrl('d'), &mut model, &mut view, &tx, 24);
        assert!(view.sidebar);
    }
    #[test]
    fn spring_entrance_is_clipped_then_overshoots_without_moving_status() {
        let model = history_model();
        let config = Config::parse_from(["recon"]);
        let mut view = View::new();
        view.sidebar = false;
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let line = |terminal: &Terminal<TestBackend>| {
            terminal
                .backend()
                .buffer()
                .content
                .chunks(100)
                .nth(3)
                .unwrap()
                .iter()
                .map(|c| c.symbol())
                .collect::<String>()
        };
        view.arrivals.insert("dir/file0.rs".into(), Instant::now());
        terminal
            .draw(|f| draw(f, &model, &mut view, &config))
            .unwrap();
        let initial = line(&terminal);
        assert!(!initial.contains("file0.rs"));
        assert!(initial.contains('M'));
        view.arrivals.insert(
            "dir/file0.rs".into(),
            Instant::now() - std::time::Duration::from_millis(400),
        );
        terminal
            .draw(|f| draw(f, &model, &mut view, &config))
            .unwrap();
        let spring = line(&terminal);
        view.arrivals.clear();
        terminal
            .draw(|f| draw(f, &model, &mut view, &config))
            .unwrap();
        let settled = line(&terminal);
        assert!(spring.find("file0.rs").unwrap() < settled.find("file0.rs").unwrap());
        assert_eq!(initial.find('M'), settled.find('M'));
        assert_eq!(spring.find('M'), settled.find('M'));
    }
    #[test]
    fn deletion_reveals_a_stationary_muted_row_beneath_the_departing_label() {
        use std::time::Duration;

        for theme in ["dark", "light", "ashen"] {
            let mut config = Config::parse_from(["recon", "--theme", theme]);
            config.animation_ms = 100_000;
            let duration = Duration::from_millis(config.animation_ms);
            let mut model = history_model();
            let file = model.files.get_mut("dir/file0.rs").unwrap();
            file.status = Status::Deleted;
            file.git = " D".into();
            file.activity = Instant::now() - duration.mul_f64(0.3);
            let mut view = View::new();
            view.sidebar = false;
            view.selected = Some("dir/file0.rs".into());
            let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
            terminal
                .draw(|f| draw(f, &model, &mut view, &config))
                .unwrap();
            let row = terminal
                .backend()
                .buffer()
                .content
                .chunks(100)
                .nth(3)
                .unwrap();
            let text: String = row.iter().map(|cell| cell.symbol()).collect();
            let base = text.find("− file0.rs").unwrap();
            assert!(text[base + "− file0.rs".len()..].contains("− file0.rs"));
            let base_cell = row.iter().position(|cell| cell.symbol() == "−").unwrap();
            for cell in &row[base_cell..base_cell + 10] {
                assert_eq!(cell.fg, config.theme.palette().muted);
            }
            let moving_cell = row.iter().rposition(|cell| cell.symbol() == "−").unwrap();
            assert!(moving_cell > base_cell);
            assert_ne!(row[moving_cell].fg, config.theme.palette().muted);
            assert!(row[moving_cell].modifier.contains(Modifier::UNDERLINED));
            assert!(row.iter().all(|cell| cell.bg == Color::Reset));

            // Also exercise the normal split layout: the coloured copy must
            // be clipped to the tree without disturbing the grey row or panel.
            view.sidebar = true;
            terminal
                .draw(|f| draw(f, &model, &mut view, &config))
                .unwrap();
            let row = terminal
                .backend()
                .buffer()
                .content
                .chunks(100)
                .nth(3)
                .unwrap();
            for cell in &row[base_cell..base_cell + 10] {
                assert_eq!(cell.fg, config.theme.palette().muted);
            }
            let divider = view.divider.unwrap() as usize;
            assert!(
                row[base_cell + 10..divider - 1]
                    .iter()
                    .any(|cell| cell.fg != config.theme.palette().muted && cell.symbol() != " ")
            );
            view.sidebar = false;

            for (no_animations, reduced_motion, age) in [
                (false, false, duration),
                (true, false, Duration::ZERO),
                (false, true, Duration::ZERO),
            ] {
                config.no_animations = no_animations;
                config.reduced_motion = reduced_motion;
                model.files.get_mut("dir/file0.rs").unwrap().activity = Instant::now() - age;
                terminal
                    .draw(|f| draw(f, &model, &mut view, &config))
                    .unwrap();
                let row = terminal
                    .backend()
                    .buffer()
                    .content
                    .chunks(100)
                    .nth(3)
                    .unwrap();
                let text: String = row.iter().map(|cell| cell.symbol()).collect();
                assert_eq!(text.matches("− file0.rs").count(), 1);
                assert_eq!(row[base_cell].symbol(), "−");
                assert_eq!(row[base_cell].fg, config.theme.palette().muted);
            }
        }
    }
    #[test]
    fn dissolving_glyphs_reveal_the_underlay_and_preserve_unicode_clusters() {
        let area = Rect::new(0, 0, 30, 1);
        let style = Style::default().fg(Color::Red);
        let mut base = Buffer::empty(area);
        Line::from(Span::styled(
            "abcdefghijklmnopqrstuvwxyz",
            Style::default().fg(Color::Gray),
        ))
        .render(area, &mut base);
        let label = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
        let mut previous_count = 27;
        for progress in [0.0, 0.5, 0.65, 0.8, 0.96] {
            let mut buffer = base.clone();
            render_departing_label(label, style, progress, area, &mut buffer);
            let visible = buffer
                .content
                .iter()
                .filter(|cell| cell.fg == Color::Red)
                .count();
            assert!(visible < previous_count);
            if progress == 0.0 {
                assert_eq!(visible, 26);
            } else if progress < 0.95 {
                assert!(visible > 0);
            } else {
                assert_eq!(buffer, base);
            }
            for (cell, underneath) in buffer.content.iter().zip(&base.content) {
                if cell.fg != Color::Red {
                    assert_eq!(cell, underneath);
                }
            }
            previous_count = visible;
        }
        let mut buffer = base.clone();
        render_departing_label("界e\u{301}", style, 0.0, area, &mut buffer);
        assert_eq!(buffer[(0, 0)].symbol(), "界");
        assert_eq!(buffer[(2, 0)].symbol(), "e\u{301}");
        let narrow = Rect::new(0, 0, 1, 1);
        let mut clipped = base.clone();
        render_departing_label("界", style, 0.0, narrow, &mut clipped);
        assert_eq!(clipped, base);
    }

    #[test]
    fn newest_entries_are_at_top_and_prepending_preserves_paused_reading() {
        let mut model = history_model();
        model.refreshing = false;
        model.branch = "feature/top-first".into();
        let mut view = View::new();
        view.following = false;
        view.panel_focus = true;
        view.scroll = 4;
        save_anchor(&model, &mut view);
        assert_eq!(view.anchor, Some((6, 0)));
        let mut entry = model.entries.back().unwrap().clone();
        entry.id = 8;
        entry.path = "dir/newest.rs".into();
        let mut row = model.files["dir/file7.rs"].clone();
        row.path = entry.path.clone();
        model.apply(crate::model::Update::Files(vec![(row, Some(entry))]));
        restore_anchor(&model, &mut view);
        assert_eq!(view.scroll, 8);
        save_anchor(&model, &mut view);
        assert_eq!(view.anchor, Some((6, 0)));
        let config = Config::parse_from(["recon", "--no-animations"]);
        let mut terminal = Terminal::new(TestBackend::new(140, 24)).unwrap();
        let (tx, _rx) = std::sync::mpsc::sync_channel(8);
        handle_key(KeyCode::Char('f').into(), &mut model, &mut view, &tx, 24);
        terminal
            .draw(|f| draw(f, &model, &mut view, &config))
            .unwrap();
        assert_eq!(view.scroll, 0);
        let header = terminal
            .backend()
            .buffer()
            .content
            .chunks(140)
            .nth(2)
            .unwrap()
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(header.contains("dir/newest.rs"));
        let top = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .take(140)
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(top.find("WATCHING").unwrap() < top.find("feature/top-first").unwrap());
        handle_key(KeyCode::Enter.into(), &mut model, &mut view, &tx, 24);
        assert!(model.entries.back().unwrap().collapsed);
        assert!(!model.entries.front().unwrap().collapsed);
    }
}
