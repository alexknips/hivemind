use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use crate::error::CliError;
use crate::projector::{GraphView, NodeKind, RelationKind};
use crate::queries::{
    get_decision, get_decision_neighborhood, get_supersession_chain, search_decisions,
    DecisionSearchResult, DecisionStatus, DecisionView, NeighborhoodRequest, NeighborhoodView,
    SearchDecisionRequest,
};
use crate::Result;

#[derive(Clone, Debug)]
pub struct TuiConfig {
    pub query: Option<String>,
    pub topic_keys: Vec<String>,
    pub statuses: Vec<DecisionStatus>,
    pub actor_ids: Vec<String>,
    pub sources: Vec<String>,
    pub limit: usize,
    pub dot_output: PathBuf,
}

pub fn run(graph: &impl GraphView, config: TuiConfig) -> Result<()> {
    let _terminal_guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal =
        Terminal::new(backend).map_err(|error| tui_io_error("open terminal", error))?;

    let mut app = DecisionSearchApp::new(config);
    app.refresh(graph);

    loop {
        terminal
            .draw(|frame| render(frame, &app))
            .map_err(|error| tui_io_error("draw terminal frame", error))?;

        if event::poll(Duration::from_millis(200))
            .map_err(|error| tui_io_error("poll terminal events", error))?
        {
            let Event::Key(key) =
                event::read().map_err(|error| tui_io_error("read terminal event", error))?
            else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if app.handle_key(key, graph) {
                break;
            }
        }
    }

    terminal
        .show_cursor()
        .map_err(|error| tui_io_error("restore terminal cursor", error))?;
    Ok(())
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().map_err(|error| tui_io_error("enable terminal raw mode", error))?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)
            .map_err(|error| tui_io_error("enter alternate screen", error))?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(stdout, LeaveAlternateScreen);
    }
}

#[derive(Clone, Debug)]
struct DecisionSearchApp {
    query_input: String,
    topic_input: String,
    actor_input: String,
    source_filters: Vec<String>,
    statuses: Vec<DecisionStatus>,
    limit: usize,
    dot_output: PathBuf,
    results: Vec<DecisionSearchResult>,
    selected_result: usize,
    detail: Option<DecisionView>,
    neighborhood: Option<NeighborhoodView>,
    neighborhood_truncated: bool,
    total_matches: usize,
    cursor: Option<String>,
    previous_cursors: Vec<Option<String>>,
    next_cursor: Option<String>,
    result_truncated: bool,
    focus: FocusPane,
    input_mode: Option<InputMode>,
    help_open: bool,
    graph_selected: usize,
    relation_focus: Option<Vec<RelationKind>>,
    breadcrumbs: Vec<String>,
    status_message: Option<String>,
    error_message: Option<String>,
}

impl DecisionSearchApp {
    fn new(config: TuiConfig) -> Self {
        Self {
            query_input: config.query.unwrap_or_default(),
            topic_input: config.topic_keys.join(","),
            actor_input: config.actor_ids.join(","),
            source_filters: normalized_csv_values(&config.sources.join(",")),
            statuses: config.statuses,
            limit: config.limit,
            dot_output: config.dot_output,
            results: Vec::new(),
            selected_result: 0,
            detail: None,
            neighborhood: None,
            neighborhood_truncated: false,
            total_matches: 0,
            cursor: None,
            previous_cursors: Vec::new(),
            next_cursor: None,
            result_truncated: false,
            focus: FocusPane::Results,
            input_mode: None,
            help_open: false,
            graph_selected: 0,
            relation_focus: None,
            breadcrumbs: Vec::new(),
            status_message: None,
            error_message: None,
        }
    }

    fn handle_key(&mut self, key: KeyEvent, graph: &impl GraphView) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
            return true;
        }

        if self.help_open {
            match key.code {
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => self.help_open = false,
                _ => {}
            }
            return false;
        }

        if self.input_mode.is_some() {
            self.handle_input_key(key, graph);
            return false;
        }

        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Char('?') => self.help_open = true,
            KeyCode::Char('/') => self.start_input(InputMode::Search),
            KeyCode::Char('t') => self.start_input(InputMode::Topic),
            KeyCode::Char('a') => self.start_input(InputMode::Actor),
            KeyCode::Char('o') => {
                self.cycle_status_filter();
                self.reset_pagination();
                self.refresh(graph);
            }
            KeyCode::Tab => self.focus = self.focus.next(),
            KeyCode::BackTab => self.focus = self.focus.previous(),
            KeyCode::PageDown | KeyCode::Char('n') => self.next_page(graph),
            KeyCode::PageUp | KeyCode::Char('N') => self.previous_page(graph),
            KeyCode::Down | KeyCode::Char('j') => self.move_down(graph),
            KeyCode::Up | KeyCode::Char('k') => self.move_up(graph),
            KeyCode::Enter => self.open_focused(graph),
            KeyCode::Char('b') => self.go_back(graph),
            KeyCode::Char('[') => {
                self.open_supersession_neighbor(graph, SupersessionDirection::Older)
            }
            KeyCode::Char(']') => {
                self.open_supersession_neighbor(graph, SupersessionDirection::Newer)
            }
            KeyCode::Char('e') => self.focus_relation_group(&[
                RelationKind::BasedOn,
                RelationKind::Supports,
                RelationKind::Refutes,
            ]),
            KeyCode::Char('h') => self.focus_relation_group(&[
                RelationKind::Assumes,
                RelationKind::Supports,
                RelationKind::Refutes,
            ]),
            KeyCode::Char('p') => self.focus_relation_group(&[
                RelationKind::ProposedBy,
                RelationKind::AcceptedBy,
                RelationKind::RejectedBy,
            ]),
            KeyCode::Char('x') => self.export_current_neighborhood(),
            KeyCode::Esc => {
                self.input_mode = None;
                self.relation_focus = None;
                self.status_message = Some("Cleared graph relation focus".to_owned());
            }
            _ => {}
        }

        false
    }

    fn handle_input_key(&mut self, key: KeyEvent, graph: &impl GraphView) {
        match key.code {
            KeyCode::Enter => {
                let mode = self.input_mode.take();
                self.selected_result = 0;
                self.reset_pagination();
                self.refresh(graph);
                self.status_message = mode.map(|mode| format!("Applied {} filter", mode.label()));
            }
            KeyCode::Esc => {
                self.input_mode = None;
                self.status_message = Some("Left input mode".to_owned());
            }
            KeyCode::Backspace => {
                self.active_input_mut().pop();
            }
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.active_input_mut().push(ch);
            }
            _ => {}
        }
    }

    fn active_input_mut(&mut self) -> &mut String {
        match self.input_mode.unwrap_or(InputMode::Search) {
            InputMode::Search => &mut self.query_input,
            InputMode::Topic => &mut self.topic_input,
            InputMode::Actor => &mut self.actor_input,
        }
    }

    fn start_input(&mut self, mode: InputMode) {
        self.input_mode = Some(mode);
        self.focus = FocusPane::Results;
        self.status_message = Some(format!(
            "Editing {}; enter applies, esc cancels",
            mode.label()
        ));
    }

    fn refresh(&mut self, graph: &impl GraphView) {
        let request = self.search_request();
        match search_decisions(graph, &request) {
            Ok(response) => {
                self.error_message = None;
                self.result_truncated = response.truncated;
                self.total_matches = response.data.total_matches;
                self.next_cursor = response.data.next_cursor;
                self.results = response.data.items;
                if self.results.is_empty() {
                    self.selected_result = 0;
                    self.detail = None;
                    self.neighborhood = None;
                    self.neighborhood_truncated = false;
                    self.graph_selected = 0;
                } else {
                    self.selected_result = self.selected_result.min(self.results.len() - 1);
                    self.load_selected_decision(graph, false);
                }
            }
            Err(error) => {
                self.error_message = Some(error.to_string());
                self.status_message = Some("Search failed; adjust filters or retry".to_owned());
            }
        }
    }

    fn search_request(&self) -> SearchDecisionRequest {
        SearchDecisionRequest {
            query: trimmed_optional_string(&self.query_input),
            topic_keys: normalized_csv_values(&self.topic_input),
            statuses: self.statuses.clone(),
            actor_ids: normalized_csv_values(&self.actor_input),
            sources: self.source_filters.clone(),
            limit: self.limit,
            cursor: self.cursor.clone(),
        }
    }

    fn load_selected_decision(&mut self, graph: &impl GraphView, push_breadcrumb: bool) {
        let Some(id) = self.selected_result_id() else {
            return;
        };
        self.open_decision_id(graph, &id, push_breadcrumb);
    }

    fn selected_result_id(&self) -> Option<String> {
        self.results
            .get(self.selected_result)
            .map(|result| result.decision.id.clone())
    }

    fn current_decision_id(&self) -> Option<&str> {
        self.detail.as_ref().map(|detail| detail.id.as_str())
    }

    fn open_decision_id(&mut self, graph: &impl GraphView, id: &str, push_breadcrumb: bool) {
        let previous = self.current_decision_id().map(str::to_owned);
        let detail = match get_decision(graph, id) {
            Ok(response) => response.data,
            Err(error) => {
                self.error_message = Some(error.to_string());
                return;
            }
        };
        let Some(detail) = detail else {
            self.error_message = Some(format!("Decision not found: {id}"));
            self.status_message =
                Some("Selected graph node is missing from the decision index".to_owned());
            return;
        };
        let neighborhood = match get_decision_neighborhood(graph, id, &NeighborhoodRequest::all()) {
            Ok(response) => {
                self.neighborhood_truncated = response.truncated;
                response.data
            }
            Err(error) => {
                self.error_message = Some(error.to_string());
                return;
            }
        };

        if push_breadcrumb && previous.as_deref().is_some_and(|previous| previous != id) {
            if let Some(previous) = previous {
                self.breadcrumbs.push(previous);
            }
        }
        self.detail = Some(detail);
        self.neighborhood = Some(neighborhood);
        self.graph_selected = 0;
        self.error_message = None;
    }

    fn move_down(&mut self, graph: &impl GraphView) {
        match self.focus {
            FocusPane::Graph => {
                let entries = self.graph_entries();
                if !entries.is_empty() {
                    self.graph_selected = (self.graph_selected + 1).min(entries.len() - 1);
                }
            }
            _ => {
                if !self.results.is_empty() {
                    self.selected_result = (self.selected_result + 1).min(self.results.len() - 1);
                    self.load_selected_decision(graph, false);
                }
            }
        }
    }

    fn move_up(&mut self, graph: &impl GraphView) {
        match self.focus {
            FocusPane::Graph => {
                self.graph_selected = self.graph_selected.saturating_sub(1);
            }
            _ => {
                if !self.results.is_empty() {
                    self.selected_result = self.selected_result.saturating_sub(1);
                    self.load_selected_decision(graph, false);
                }
            }
        }
    }

    fn open_focused(&mut self, graph: &impl GraphView) {
        match self.focus {
            FocusPane::Results | FocusPane::Detail => self.load_selected_decision(graph, true),
            FocusPane::Graph => {
                let entries = self.graph_entries();
                let Some(entry) = entries.get(self.graph_selected) else {
                    return;
                };
                match (entry.node_kind, entry.node_id.as_deref()) {
                    (Some(NodeKind::Decision), Some(id)) => self.open_decision_id(graph, id, true),
                    (Some(kind), Some(id)) => {
                        self.status_message = Some(format!(
                            "{} is a {} node; only decision nodes open in this slice",
                            id,
                            kind.table_name()
                        ));
                    }
                    _ => {
                        self.status_message =
                            Some("Graph group headers are not navigable".to_owned())
                    }
                }
            }
        }
    }

    fn go_back(&mut self, graph: &impl GraphView) {
        if let Some(previous) = self.breadcrumbs.pop() {
            self.open_decision_id(graph, &previous, false);
            self.status_message = Some(format!("Returned to {previous}"));
        } else {
            self.status_message = Some("Breadcrumb stack is empty".to_owned());
        }
    }

    fn open_supersession_neighbor(
        &mut self,
        graph: &impl GraphView,
        direction: SupersessionDirection,
    ) {
        let Some(current) = self.current_decision_id().map(str::to_owned) else {
            return;
        };
        let chain = match get_supersession_chain(graph, &current) {
            Ok(response) => response.data,
            Err(error) => {
                self.error_message = Some(error.to_string());
                self.status_message = Some("Supersession traversal failed".to_owned());
                return;
            }
        };
        let target_index = match direction {
            SupersessionDirection::Older => chain.input_index.checked_sub(1),
            SupersessionDirection::Newer => {
                let next = chain.input_index + 1;
                (next < chain.decision_ids.len()).then_some(next)
            }
        };
        let Some(target_index) = target_index else {
            self.status_message = Some(match direction {
                SupersessionDirection::Older => "No older superseded decision".to_owned(),
                SupersessionDirection::Newer => "No newer superseding decision".to_owned(),
            });
            return;
        };
        if let Some(target) = chain.decision_ids.get(target_index) {
            let target = target.clone();
            self.open_decision_id(graph, &target, true);
            self.status_message = Some(format!("Opened supersession neighbor {target}"));
        }
    }

    fn cycle_status_filter(&mut self) {
        let next = match self.statuses.as_slice() {
            [] => Some(DecisionStatus::Proposed),
            [DecisionStatus::Proposed] => Some(DecisionStatus::Accepted),
            [DecisionStatus::Accepted] => Some(DecisionStatus::Rejected),
            [DecisionStatus::Rejected] => Some(DecisionStatus::Contested),
            [DecisionStatus::Contested] => Some(DecisionStatus::Superseded),
            [DecisionStatus::Superseded] => None,
            _ => None,
        };
        self.statuses.clear();
        if let Some(status) = next {
            self.statuses.push(status);
        }
        self.status_message = Some(format!("Status filter: {}", self.status_label()));
    }

    fn reset_pagination(&mut self) {
        self.cursor = None;
        self.previous_cursors.clear();
        self.next_cursor = None;
    }

    fn next_page(&mut self, graph: &impl GraphView) {
        let Some(next_cursor) = self.next_cursor.clone() else {
            self.status_message = Some("No next result page".to_owned());
            return;
        };
        self.previous_cursors.push(self.cursor.clone());
        self.cursor = Some(next_cursor);
        self.selected_result = 0;
        self.refresh(graph);
        self.status_message = Some("Loaded next result page".to_owned());
    }

    fn previous_page(&mut self, graph: &impl GraphView) {
        let Some(previous_cursor) = self.previous_cursors.pop() else {
            self.status_message = Some("No previous result page".to_owned());
            return;
        };
        self.cursor = previous_cursor;
        self.selected_result = 0;
        self.refresh(graph);
        self.status_message = Some("Loaded previous result page".to_owned());
    }

    fn focus_relation_group(&mut self, relations: &[RelationKind]) {
        self.focus = FocusPane::Graph;
        self.relation_focus = Some(relations.to_vec());
        let entries = self.graph_entries();
        if let Some((index, _)) = entries.iter().enumerate().find(|(_, entry)| {
            entry
                .relation
                .is_some_and(|relation| relations.contains(&relation))
        }) {
            self.graph_selected = index;
        }
        self.status_message = Some(format!(
            "Focused graph relations: {}",
            relations
                .iter()
                .map(|relation| relation.table_name())
                .collect::<Vec<_>>()
                .join(",")
        ));
    }

    fn export_current_neighborhood(&mut self) {
        let Some(neighborhood) = self.neighborhood.as_ref() else {
            self.status_message = Some("No neighborhood to export".to_owned());
            return;
        };
        let dot = render_neighborhood_dot(neighborhood);
        match std::fs::write(&self.dot_output, dot) {
            Ok(()) => {
                self.error_message = None;
                self.status_message = Some(format!(
                    "Exported focused neighborhood DOT to {}",
                    self.dot_output.display()
                ));
            }
            Err(error) => {
                self.error_message = Some(format!(
                    "DOT export failed for {}: {error}",
                    self.dot_output.display()
                ));
            }
        }
    }

    fn graph_entries(&self) -> Vec<GraphEntry> {
        let Some(neighborhood) = self.neighborhood.as_ref() else {
            return Vec::new();
        };
        graph_entries(neighborhood)
    }

    fn status_label(&self) -> String {
        if self.statuses.is_empty() {
            return "any".to_owned();
        }
        self.statuses
            .iter()
            .map(|status| decision_status_label(*status))
            .collect::<Vec<_>>()
            .join(",")
    }

    fn active_constraints(&self) -> Vec<String> {
        let mut constraints = Vec::new();
        if !self.query_input.trim().is_empty() {
            constraints.push(format!("q={}", self.query_input.trim()));
        }
        for topic in normalized_csv_values(&self.topic_input) {
            constraints.push(format!("topic={topic}"));
        }
        for status in &self.statuses {
            constraints.push(format!("status={}", decision_status_label(*status)));
        }
        for actor in normalized_csv_values(&self.actor_input) {
            constraints.push(format!("actor={actor}"));
        }
        for source in &self.source_filters {
            constraints.push(format!("source={source}"));
        }
        constraints
    }

    fn is_empty_ledger_state(&self) -> bool {
        self.results.is_empty() && self.active_constraints().is_empty()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FocusPane {
    Results,
    Detail,
    Graph,
}

impl FocusPane {
    const fn next(self) -> Self {
        match self {
            Self::Results => Self::Detail,
            Self::Detail => Self::Graph,
            Self::Graph => Self::Results,
        }
    }

    const fn previous(self) -> Self {
        match self {
            Self::Results => Self::Graph,
            Self::Detail => Self::Results,
            Self::Graph => Self::Detail,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Results => "results",
            Self::Detail => "detail",
            Self::Graph => "graph",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InputMode {
    Search,
    Topic,
    Actor,
}

impl InputMode {
    const fn label(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Topic => "topic",
            Self::Actor => "actor",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SupersessionDirection {
    Older,
    Newer,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct GraphEntry {
    label: String,
    relation: Option<RelationKind>,
    node_kind: Option<NodeKind>,
    node_id: Option<String>,
}

fn render(frame: &mut Frame<'_>, app: &DecisionSearchApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(3),
            Constraint::Length(3),
        ])
        .split(frame.area());

    let filter_area = chunks.first().copied().unwrap_or_else(|| frame.area());
    let main_area = chunks.get(1).copied().unwrap_or_else(|| frame.area());
    let status_area = chunks.get(2).copied().unwrap_or_else(|| frame.area());
    render_filter_bar(frame, filter_area, app);
    render_main(frame, main_area, app);
    render_status_bar(frame, status_area, app);

    if app.help_open {
        render_help_overlay(frame);
    }
}

fn render_filter_bar(frame: &mut Frame<'_>, area: Rect, app: &DecisionSearchApp) {
    let query = input_with_cursor(&app.query_input, app.input_mode == Some(InputMode::Search));
    let topic = input_with_cursor(&app.topic_input, app.input_mode == Some(InputMode::Topic));
    let actor = input_with_cursor(&app.actor_input, app.input_mode == Some(InputMode::Actor));
    let sources = if app.source_filters.is_empty() {
        "any".to_owned()
    } else {
        app.source_filters.join(",")
    };
    let mode = app
        .input_mode
        .map(|mode| format!("editing {}", mode.label()))
        .unwrap_or_else(|| format!("focus {}", app.focus.label()));
    let truncation = if app.result_truncated {
        format!(" | next {}", app.next_cursor.as_deref().unwrap_or("?"))
    } else {
        String::new()
    };
    let cursor = app.cursor.as_deref().unwrap_or("start");
    let lines = vec![
        Line::from(vec![
            Span::styled("/ ", Style::default().fg(Color::Cyan)),
            Span::raw(query),
            Span::raw(" | topic "),
            Span::raw(topic),
            Span::raw(" | status "),
            Span::raw(app.status_label()),
            Span::raw(" | actor "),
            Span::raw(actor),
            Span::raw(" | source "),
            Span::raw(sources),
        ]),
        Line::from(format!(
            "{mode} | results {}/{} | cursor {cursor}{} | n next | N previous | ? help | q quit",
            app.results.len(),
            app.total_matches,
            truncation
        )),
    ];
    let paragraph = Paragraph::new(lines)
        .block(Block::default().title("Filters").borders(Borders::ALL))
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}

fn render_main(frame: &mut Frame<'_>, area: Rect, app: &DecisionSearchApp) {
    if area.width < 100 {
        match app.focus {
            FocusPane::Results => render_results(frame, area, app),
            FocusPane::Detail => render_detail(frame, area, app),
            FocusPane::Graph => render_graph(frame, area, app),
        }
        return;
    }

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(34), Constraint::Percentage(66)])
        .split(area);
    let left_area = columns.first().copied().unwrap_or(area);
    let right_area = columns.get(1).copied().unwrap_or(area);
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(right_area);
    let detail_area = right.first().copied().unwrap_or(right_area);
    let graph_area = right.get(1).copied().unwrap_or(right_area);
    render_results(frame, left_area, app);
    render_detail(frame, detail_area, app);
    render_graph(frame, graph_area, app);
}

fn render_results(frame: &mut Frame<'_>, area: Rect, app: &DecisionSearchApp) {
    if app.results.is_empty() {
        let message = if app.is_empty_ledger_state() {
            vec![
                Line::from("Empty ledger."),
                Line::from("Seed: cargo test --test seed -- --include-ignored"),
                Line::from("Emit: hivemind emit decision.proposed --title ... --rationale ..."),
            ]
        } else {
            let constraints = app.active_constraints();
            vec![
                Line::from("No search results."),
                Line::from(format!(
                    "Constraints: {}",
                    if constraints.is_empty() {
                        "none".to_owned()
                    } else {
                        constraints.join(", ")
                    }
                )),
            ]
        };
        let paragraph = Paragraph::new(message)
            .block(focused_block("Results", app.focus == FocusPane::Results))
            .wrap(Wrap { trim: true });
        frame.render_widget(paragraph, area);
        return;
    }

    let items = app
        .results
        .iter()
        .map(|result| {
            let stale = if result.decision.hypotheses.iter().any(|hypothesis| {
                matches!(hypothesis.status, crate::queries::HypothesisStatus::Refuted)
            }) {
                " !refuted"
            } else {
                ""
            };
            let topics = if result.decision.topic_keys.is_empty() {
                String::new()
            } else {
                format!(" [{}]", result.decision.topic_keys.join(","))
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{} ", decision_status_label(result.decision.status)),
                    status_style(result.decision.status),
                ),
                Span::styled(&result.decision.id, Style::default().fg(Color::Cyan)),
                Span::raw(format!(" {}{}{}", result.decision.title, topics, stale)),
            ]))
        })
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(focused_block("Results", app.focus == FocusPane::Results))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    let mut state = ListState::default();
    state.select(Some(app.selected_result));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_detail(frame: &mut Frame<'_>, area: Rect, app: &DecisionSearchApp) {
    let mut lines = Vec::new();
    if let Some(detail) = app.detail.as_ref() {
        lines.push(Line::from(vec![
            Span::styled(
                &detail.id,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                decision_status_label(detail.status),
                status_style(detail.status),
            ),
        ]));
        lines.push(Line::from(detail.title.clone()));
        lines.push(Line::from(format!(
            "topics: {}",
            list_or_none(&detail.topic_keys)
        )));
        lines.push(Line::from(""));
        lines.push(Line::from("rationale:"));
        lines.extend(wrapped_lines(&detail.rationale));
        lines.push(Line::from(""));
        lines.push(Line::from(format!(
            "chosen option: {}",
            detail.chosen_option_id.as_deref().unwrap_or("none")
        )));
        lines.push(Line::from(format!(
            "other options: {}",
            list_or_none(&detail.option_ids)
        )));
        lines.push(Line::from(format!(
            "evidence: {}",
            list_or_none(&detail.evidence_ids)
        )));
        let hypotheses = detail
            .hypotheses
            .iter()
            .map(|hypothesis| {
                format!(
                    "{} ({})",
                    hypothesis.id,
                    hypothesis_status_label(hypothesis.status)
                )
            })
            .collect::<Vec<_>>();
        lines.push(Line::from(format!(
            "hypotheses: {}",
            list_or_none(&hypotheses)
        )));

        let actors = actor_edges(app.neighborhood.as_ref());
        lines.push(Line::from(format!("actors: {}", list_or_none(&actors))));
        let supersession = supersession_summary(app.neighborhood.as_ref(), &detail.id);
        lines.push(Line::from(format!("supersession: {supersession}")));
        if detail.hypotheses.iter().any(|hypothesis| {
            matches!(hypothesis.status, crate::queries::HypothesisStatus::Refuted)
        }) {
            lines.push(Line::from(vec![Span::styled(
                "warning: one or more assumed hypotheses are refuted",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )]));
        }
        if !app.breadcrumbs.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(format!(
                "breadcrumbs: {} -> {}",
                app.breadcrumbs.join(" -> "),
                detail.id
            )));
        }
    } else if app.is_empty_ledger_state() {
        lines.push(Line::from("Empty ledger."));
        lines.push(Line::from(
            "Seed: cargo test --test seed -- --include-ignored",
        ));
        lines.push(Line::from(
            "Or emit the first decision with hivemind emit decision.proposed.",
        ));
    } else {
        lines.push(Line::from("No selected decision."));
        lines.push(Line::from("Adjust filters or clear search constraints."));
    }

    let paragraph = Paragraph::new(lines)
        .block(focused_block(
            "Decision Detail",
            app.focus == FocusPane::Detail,
        ))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn render_graph(frame: &mut Frame<'_>, area: Rect, app: &DecisionSearchApp) {
    let entries = app.graph_entries();
    if entries.is_empty() {
        let message = if let Some(neighborhood) = app.neighborhood.as_ref() {
            if !neighborhood.root.present {
                format!("Missing graph projection for {}", neighborhood.root.id)
            } else {
                "No one-hop graph context for this decision".to_owned()
            }
        } else {
            "No graph context loaded".to_owned()
        };
        let paragraph = Paragraph::new(vec![Line::from(message)])
            .block(focused_block(
                "Graph Context",
                app.focus == FocusPane::Graph,
            ))
            .wrap(Wrap { trim: true });
        frame.render_widget(paragraph, area);
        return;
    }

    let items = entries
        .iter()
        .map(|entry| {
            let style = if entry.node_kind == Some(NodeKind::Decision) {
                Style::default().fg(Color::Cyan)
            } else if entry.node_kind.is_some() {
                Style::default().fg(Color::Gray)
            } else {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            };
            ListItem::new(Line::from(Span::styled(entry.label.clone(), style)))
        })
        .collect::<Vec<_>>();
    let title = if app.neighborhood_truncated {
        "Graph Context (truncated)"
    } else {
        "Graph Context"
    };
    let list = List::new(items)
        .block(focused_block(title, app.focus == FocusPane::Graph))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    let mut state = ListState::default();
    state.select(Some(
        app.graph_selected.min(entries.len().saturating_sub(1)),
    ));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_status_bar(frame: &mut Frame<'_>, area: Rect, app: &DecisionSearchApp) {
    let message = app
        .error_message
        .as_ref()
        .map(|message| format!("error: {message}"))
        .or_else(|| app.status_message.clone())
        .unwrap_or_else(|| {
            "j/k move | n/N page | enter open | b back | [/] supersession | e/h/p graph | x DOT"
                .to_owned()
        });
    let style = if app.error_message.is_some() {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::Gray)
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(message, style)))
            .block(Block::default().borders(Borders::ALL))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_help_overlay(frame: &mut Frame<'_>) {
    let area = centered_rect(72, 20, frame.area());
    frame.render_widget(Clear, area);
    let lines = vec![
        Line::from("Keyboard"),
        Line::from("j/k or arrows: move selection"),
        Line::from("/: edit search text | t: topic filter | a: actor filter | o: cycle status"),
        Line::from("n/PageDown: next page | N/PageUp: previous page when results are truncated"),
        Line::from("tab: rotate results/detail/graph focus | enter: open selected"),
        Line::from("b: back through breadcrumbs | [ and ]: supersession neighbors"),
        Line::from("e/h/p: focus evidence, hypotheses, or provenance edges"),
        Line::from("x: export focused neighborhood as DOT"),
        Line::from("esc: leave input or close focus | q: quit"),
        Line::from(""),
        Line::from("Read-only: this TUI calls query APIs and never emits ledger events."),
    ];
    let paragraph = Paragraph::new(lines)
        .block(Block::default().title("Help").borders(Borders::ALL))
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}

fn graph_entries(neighborhood: &NeighborhoodView) -> Vec<GraphEntry> {
    let node_kinds = neighborhood
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node.kind))
        .collect::<BTreeMap<_, _>>();
    let mut entries = Vec::new();
    for relation in RelationKind::ALL {
        let edges = neighborhood
            .edges
            .iter()
            .filter(|edge| edge.relation == relation)
            .collect::<Vec<_>>();
        if edges.is_empty() {
            continue;
        }
        entries.push(GraphEntry {
            label: format!("{} ({})", relation.table_name(), edges.len()),
            relation: Some(relation),
            node_kind: None,
            node_id: None,
        });
        for edge in edges {
            let (node_kind, node_id) = navigable_endpoint(
                &node_kinds,
                &neighborhood.root.id,
                edge.from.as_str(),
                edge.to.as_str(),
            );
            entries.push(GraphEntry {
                label: format!("  {} -> {}", edge.from, edge.to),
                relation: Some(relation),
                node_kind,
                node_id: node_id.map(str::to_owned),
            });
        }
    }
    entries
}

fn navigable_endpoint<'a>(
    node_kinds: &BTreeMap<&'a str, NodeKind>,
    root_id: &str,
    from: &'a str,
    to: &'a str,
) -> (Option<NodeKind>, Option<&'a str>) {
    if from != root_id {
        if let Some(kind) = node_kinds.get(from).copied() {
            return (Some(kind), Some(from));
        }
    }
    if to != root_id {
        if let Some(kind) = node_kinds.get(to).copied() {
            return (Some(kind), Some(to));
        }
    }
    if let Some(kind) = node_kinds.get(to).copied() {
        return (Some(kind), Some(to));
    }
    (node_kinds.get(from).copied(), Some(from))
}

pub fn render_neighborhood_dot(neighborhood: &NeighborhoodView) -> String {
    let mut dot = String::from("digraph hivemind_neighborhood {\n  rankdir=LR;\n");
    let nodes = neighborhood
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<BTreeMap<_, _>>();
    for node in &neighborhood.nodes {
        let mut label = format!("{}:{}", node.kind.table_name(), node.id);
        if let Some(status) = node.decision_status {
            label.push_str(&format!("\\nstatus: {}", decision_status_label(status)));
        }
        if let Some(status) = node.hypothesis_status {
            label.push_str(&format!("\\nstatus: {}", hypothesis_status_label(status)));
        }
        dot.push_str(&format!(
            "  \"{}\" [label=\"{}\", shape=box];\n",
            dot_node_key(node.kind, &node.id),
            escape_dot(&label)
        ));
    }
    for edge in &neighborhood.edges {
        let from_kind = nodes
            .get(edge.from.as_str())
            .map(|node| node.kind)
            .unwrap_or(NodeKind::Decision);
        let to_kind = nodes
            .get(edge.to.as_str())
            .map(|node| node.kind)
            .unwrap_or(NodeKind::Decision);
        dot.push_str(&format!(
            "  \"{}\" -> \"{}\" [label=\"{}\"];\n",
            dot_node_key(from_kind, &edge.from),
            dot_node_key(to_kind, &edge.to),
            edge.relation.table_name()
        ));
    }
    dot.push_str("}\n");
    dot
}

fn actor_edges(neighborhood: Option<&NeighborhoodView>) -> Vec<String> {
    let Some(neighborhood) = neighborhood else {
        return Vec::new();
    };
    neighborhood
        .edges
        .iter()
        .filter(|edge| {
            matches!(
                edge.relation,
                RelationKind::ProposedBy | RelationKind::AcceptedBy | RelationKind::RejectedBy
            )
        })
        .map(|edge| format!("{}:{}", edge.relation.table_name(), edge.to))
        .collect()
}

fn supersession_summary(neighborhood: Option<&NeighborhoodView>, current_id: &str) -> String {
    let Some(neighborhood) = neighborhood else {
        return "none".to_owned();
    };
    let older = neighborhood
        .edges
        .iter()
        .filter(|edge| edge.relation == RelationKind::Supersedes && edge.from == current_id)
        .map(|edge| edge.to.clone())
        .collect::<Vec<_>>();
    let newer = neighborhood
        .edges
        .iter()
        .filter(|edge| edge.relation == RelationKind::Supersedes && edge.to == current_id)
        .map(|edge| edge.from.clone())
        .collect::<Vec<_>>();
    match (older.is_empty(), newer.is_empty()) {
        (true, true) => "none".to_owned(),
        (false, true) => format!("supersedes {}", older.join(",")),
        (true, false) => format!("superseded by {}", newer.join(",")),
        (false, false) => format!(
            "supersedes {}; superseded by {}",
            older.join(","),
            newer.join(",")
        ),
    }
}

fn focused_block(title: &'static str, focused: bool) -> Block<'static> {
    let style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::Gray)
    };
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(style)
}

fn status_style(status: DecisionStatus) -> Style {
    match status {
        DecisionStatus::Proposed => Style::default().fg(Color::Gray),
        DecisionStatus::Accepted => Style::default().fg(Color::Green),
        DecisionStatus::Rejected => Style::default().fg(Color::Red),
        DecisionStatus::Contested => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        DecisionStatus::Superseded => Style::default().fg(Color::Magenta),
    }
}

fn decision_status_label(status: DecisionStatus) -> &'static str {
    match status {
        DecisionStatus::Proposed => "proposed",
        DecisionStatus::Accepted => "accepted",
        DecisionStatus::Rejected => "rejected",
        DecisionStatus::Contested => "contested",
        DecisionStatus::Superseded => "superseded",
    }
}

fn hypothesis_status_label(status: crate::queries::HypothesisStatus) -> &'static str {
    match status {
        crate::queries::HypothesisStatus::Open => "open",
        crate::queries::HypothesisStatus::Supported => "supported",
        crate::queries::HypothesisStatus::Refuted => "refuted",
    }
}

fn normalized_csv_values(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn trimmed_optional_string(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn input_with_cursor(value: &str, active: bool) -> String {
    if active {
        format!("{value}_")
    } else if value.trim().is_empty() {
        "any".to_owned()
    } else {
        value.to_owned()
    }
}

fn wrapped_lines(value: &str) -> Vec<Line<'static>> {
    if value.is_empty() {
        return vec![Line::from("  none")];
    }
    value
        .split('\n')
        .map(|line| Line::from(format!("  {line}")))
        .collect()
}

fn list_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_owned()
    } else {
        values.join(", ")
    }
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let max_width = area.width.saturating_sub(2).max(1);
    let max_height = area.height.saturating_sub(2).max(1);
    let width = width.min(max_width).max(1);
    let height = height.min(max_height).max(1);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn dot_node_key(kind: NodeKind, id: &str) -> String {
    format!("{}:{}", kind.table_name(), id)
}

fn escape_dot(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn tui_io_error(context: &str, error: impl std::fmt::Display) -> crate::HivemindError {
    CliError::InvalidInput(format!("{context}: {error}")).into()
}

#[cfg(test)]
mod tests;
