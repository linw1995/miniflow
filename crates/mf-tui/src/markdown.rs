//! Cached Markdown presentation, independent of chat persistence and terminal I/O.

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use std::{ops::Range, sync::LazyLock};
use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, HighlightState, Theme, ThemeSet},
    parsing::{ParseState, SyntaxSet},
};

const MAX_HIGHLIGHT_LINE_BYTES: usize = 16 * 1024;
static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEME: LazyLock<Theme> = LazyLock::new(|| {
    ThemeSet::load_defaults()
        .themes
        .remove("base16-ocean.dark")
        .unwrap()
});

pub fn safe(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '\t' {
            result.push_str("    ");
        } else if c.is_control() && c != '\n' {
            result.extend(c.escape_default());
        } else {
            result.push(c);
        }
    }
    result
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub block_start: usize,
    pub line: usize,
    pub byte: usize,
    pub gap: bool,
}

pub struct VisualRow {
    pub byte: usize,
    pub line: Line<'static>,
}

#[derive(Default)]
struct CachedLine {
    text: Line<'static>,
    width: Option<u16>,
    rows: Vec<VisualRow>,
}

impl CachedLine {
    fn layout(&mut self, width: u16) {
        if self.width != Some(width) {
            self.rows = wrap(&self.text, width);
            self.width = Some(width);
        }
    }
}

#[derive(Default)]
struct Block {
    source: Range<usize>,
    events: Vec<Event<'static>>,
    code: Vec<CodeCache>,
    lines: Vec<CachedLine>,
    height: usize,
    width: Option<u16>,
    dirty: bool,
    line_ends: Vec<usize>,
}

impl Block {
    fn update(&mut self, events: Vec<Event<'static>>) {
        if self.events == events {
            return;
        }
        let lines = render(&events, &mut self.code);
        self.events = events;
        self.set_lines(lines);
    }

    fn set_lines(&mut self, lines: Vec<Line<'static>>) {
        self.dirty |= self.lines.len() != lines.len();
        self.lines.truncate(lines.len());
        for (index, text) in lines.into_iter().enumerate() {
            if let Some(line) = self.lines.get_mut(index) {
                if line.text != text {
                    self.dirty = true;
                    *line = CachedLine {
                        text,
                        ..Default::default()
                    };
                }
            } else {
                self.dirty = true;
                self.lines.push(CachedLine {
                    text,
                    ..Default::default()
                });
            }
        }
    }

    fn layout(&mut self, width: u16) {
        if self.width == Some(width) && !self.dirty {
            return;
        }
        self.width = Some(width);
        self.dirty = false;
        self.height = 0;
        self.line_ends.clear();
        for line in &mut self.lines {
            line.layout(width);
            self.height += line.rows.len();
            self.line_ends.push(self.height);
        }
    }
}

#[derive(Default)]
pub struct Document {
    source: Option<String>,
    literal_style: Option<Style>,
    blocks: Vec<Block>,
    ends: Vec<usize>,
    #[cfg(test)]
    parses: usize,
}

impl Document {
    pub fn update(&mut self, source: &str) {
        if self.source.as_deref() == Some(source) && self.literal_style.is_none() {
            return;
        }
        self.source = Some(source.to_owned());
        self.literal_style = None;
        #[cfg(test)]
        {
            self.parses += 1;
        }
        // Parse the whole changed message so late reference definitions and container
        // changes invalidate earlier blocks correctly. Reuse semantic blocks afterwards.
        let parser = Parser::new_ext(
            source,
            Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_TABLES,
        );
        let mut depth = 0usize;
        let mut events = vec![];
        let mut range = 0..0;
        let mut index = 0;
        for (event, span) in parser.into_offset_iter() {
            if events.is_empty() {
                range = span.clone();
            }
            range.end = range.end.max(span.end);
            match &event {
                Event::Start(_) => depth += 1,
                Event::End(_) => depth = depth.saturating_sub(1),
                _ => {}
            }
            events.push(event.into_static());
            if depth == 0 {
                if index == self.blocks.len() {
                    self.blocks.push(Block::default());
                }
                let block = &mut self.blocks[index];
                block.source = range.clone();
                block.update(std::mem::take(&mut events));
                index += 1;
            }
        }
        self.blocks.truncate(index);
    }

    pub fn plain(&mut self, lines: Vec<Line<'static>>) {
        self.source = None;
        self.literal_style = None;
        self.blocks.resize_with(1, Block::default);
        let block = &mut self.blocks[0];
        block.source = 0..0;
        block.events.clear();
        block.code.clear();
        block.set_lines(lines);
    }

    pub fn literal(&mut self, source: &str, style: Style) {
        if self.source.as_deref() == Some(source) && self.literal_style == Some(style) {
            return;
        }
        self.plain(
            safe(source)
                .split('\n')
                .map(|line| Line::styled(line.to_owned(), style))
                .collect(),
        );
        self.source = Some(source.to_owned());
        self.literal_style = Some(style);
    }

    pub fn layout(&mut self, width: u16) -> usize {
        self.ends.clear();
        let mut height = 0;
        for (index, block) in self.blocks.iter_mut().enumerate() {
            block.layout(width);
            height += block.height + usize::from(index > 0);
            self.ends.push(height);
        }
        height
    }

    pub fn row(&self, row: usize) -> Option<(Position, &Line<'static>)> {
        let block_index = self.ends.partition_point(|end| *end <= row);
        let block = self.blocks.get(block_index)?;
        let start = block_index.checked_sub(1).map_or(0, |i| self.ends[i]);
        let mut offset = row - start;
        if block_index > 0 {
            if offset == 0 {
                static EMPTY: LazyLock<Line<'static>> = LazyLock::new(Line::default);
                return Some((
                    Position {
                        block_start: block.source.start,
                        gap: true,
                        ..Default::default()
                    },
                    &EMPTY,
                ));
            }
            offset -= 1;
        }
        let line_index = block.line_ends.partition_point(|end| *end <= offset);
        let line = block.lines.get(line_index)?;
        let line_start = line_index.checked_sub(1).map_or(0, |i| block.line_ends[i]);
        let visual = line.rows.get(offset - line_start)?;
        Some((
            Position {
                block_start: block.source.start,
                line: line_index,
                byte: visual.byte,
                gap: false,
            },
            &visual.line,
        ))
    }

    pub fn locate(&self, position: Position) -> usize {
        let index = self
            .blocks
            .partition_point(|block| block.source.start <= position.block_start)
            .saturating_sub(1);
        let Some(block) = self.blocks.get(index) else {
            return self
                .ends
                .last()
                .copied()
                .unwrap_or_default()
                .saturating_sub(1);
        };
        let start = index.checked_sub(1).map_or(0, |i| self.ends[i]);
        if position.gap {
            return start;
        }
        let Some(line) = block.lines.get(position.line) else {
            return self.ends[index].saturating_sub(1);
        };
        start
            + usize::from(index > 0)
            + position
                .line
                .checked_sub(1)
                .map_or(0, |i| block.line_ends[i])
            + line
                .rows
                .partition_point(|row| row.byte <= position.byte)
                .saturating_sub(1)
    }
}

fn push_span(spans: &mut Vec<Span<'static>>, text: &str, style: Style) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = spans.last_mut().filter(|last| last.style == style) {
        last.content.to_mut().push_str(text);
    } else {
        spans.push(Span::styled(text.to_owned(), style));
    }
}

fn wrap(line: &Line<'static>, width: u16) -> Vec<VisualRow> {
    if width == 0 {
        return vec![];
    }
    let graphemes: Vec<_> = line.styled_graphemes(Style::default()).collect();
    let mut result = vec![];
    let mut start = 0;
    let mut byte = 0;
    while start < graphemes.len() {
        let mut end = start;
        let mut cells = 0;
        let mut boundary = None;
        let mut has_text = false;
        while end < graphemes.len() {
            let next = Span::raw(graphemes[end].symbol).width();
            if cells + next > width as usize {
                break;
            }
            cells += next;
            end += 1;
            let whitespace = graphemes[end - 1].symbol.chars().all(char::is_whitespace);
            if whitespace && has_text {
                boundary = Some(end);
            }
            has_text |= !whitespace;
        }
        if end < graphemes.len()
            && let Some(boundary) = boundary
        {
            end = boundary;
        }
        let mut spans = vec![];
        let row_byte = byte;
        if end == start {
            // A wide grapheme cannot fit in a one-cell viewport.
            push_span(&mut spans, "�", graphemes[start].style);
            byte += graphemes[start].symbol.len();
            end += 1;
        } else {
            for grapheme in &graphemes[start..end] {
                push_span(&mut spans, grapheme.symbol, grapheme.style);
                byte += grapheme.symbol.len();
            }
        }
        result.push(VisualRow {
            byte: row_byte,
            line: Line::from(spans),
        });
        start = end;
    }
    if result.is_empty() {
        result.push(VisualRow {
            byte: 0,
            line: Line::default(),
        });
    }
    result
}

#[derive(Default)]
struct CodeCache {
    language: String,
    source: String,
    committed_bytes: usize,
    committed: Vec<Line<'static>>,
    state: Option<(HighlightState, ParseState)>,
    disabled: bool,
    #[cfg(test)]
    highlighted: usize,
}

impl CodeCache {
    fn update(&mut self, language: &str, source: &str) -> Vec<Line<'static>> {
        if self.language != language || !source.starts_with(&self.source) {
            *self = Self::default();
        }
        self.language = language.to_owned();
        self.source = source.to_owned();
        let token = match language {
            "c++" => "cpp",
            "c#" => "cs",
            "shell" | "bash" | "sh" => "sh",
            "javascript" => "js",
            "typescript" => "ts",
            "python" => "py",
            "golang" => "go",
            "yml" => "yaml",
            token => token,
        };
        let syntax = SYNTAXES
            .find_syntax_by_token(token)
            .unwrap_or_else(|| SYNTAXES.find_syntax_plain_text());
        let mut highlighter = if let Some((highlight, parse)) = &self.state {
            HighlightLines::from_state(&THEME, highlight.clone(), parse.clone())
        } else {
            HighlightLines::new(syntax, &THEME)
        };
        let mut tail = None;
        for text in source[self.committed_bytes..].split_inclusive('\n') {
            let mut spans = vec![];
            if text.len() > MAX_HIGHLIGHT_LINE_BYTES {
                self.disabled = true;
            }
            if !self.disabled {
                #[cfg(test)]
                {
                    self.highlighted += 1;
                }
                match highlighter.highlight_line(text, &SYNTAXES) {
                    Ok(ranges) => {
                        for (style, token) in ranges {
                            let mut modifiers = Modifier::empty();
                            if style.font_style.contains(FontStyle::BOLD) {
                                modifiers |= Modifier::BOLD;
                            }
                            if style.font_style.contains(FontStyle::ITALIC) {
                                modifiers |= Modifier::ITALIC;
                            }
                            if style.font_style.contains(FontStyle::UNDERLINE) {
                                modifiers |= Modifier::UNDERLINED;
                            }
                            push_span(
                                &mut spans,
                                &safe(token.trim_end_matches('\n')),
                                Style::default()
                                    .fg(Color::Rgb(
                                        style.foreground.r,
                                        style.foreground.g,
                                        style.foreground.b,
                                    ))
                                    .add_modifier(modifiers),
                            );
                        }
                    }
                    Err(_) => self.disabled = true,
                }
            }
            if self.disabled {
                spans = vec![Span::raw(safe(text.trim_end_matches('\n')))];
            }
            let line = Line::from(spans);
            if text.ends_with('\n') {
                self.committed.push(line);
                self.committed_bytes += text.len();
            } else {
                tail = Some(line);
                break;
            }
            self.state = Some(highlighter.state());
            let (highlight, parse) = self.state.as_ref().unwrap();
            highlighter = HighlightLines::from_state(&THEME, highlight.clone(), parse.clone());
        }
        let mut lines = self.committed.clone();
        if let Some(tail) = tail {
            lines.push(tail);
        }
        lines
    }
}

enum Container {
    Quote,
    List(Option<u64>),
    Item { marker: String, first: bool },
}

#[derive(Default)]
struct Renderer {
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    containers: Vec<Container>,
    styles: Vec<Style>,
}

impl Renderer {
    fn style(&self) -> Style {
        self.styles.last().copied().unwrap_or_default()
    }

    fn prefix(&mut self) -> String {
        let mut prefix = String::new();
        for container in &mut self.containers {
            match container {
                Container::Quote => prefix.push_str("│ "),
                Container::Item { marker, first } => {
                    if *first {
                        prefix.push_str(marker);
                        *first = false;
                    } else {
                        prefix.push_str(&" ".repeat(marker.len()));
                    }
                }
                Container::List(_) => {}
            }
        }
        prefix
    }

    fn text(&mut self, text: &str, style: Style) {
        for (index, text) in safe(text).split('\n').enumerate() {
            if index > 0 {
                self.newline(true);
            }
            if self.current.is_empty() {
                let prefix = self.prefix();
                push_span(
                    &mut self.current,
                    &prefix,
                    Style::default().fg(Color::DarkGray),
                );
            }
            push_span(&mut self.current, text, style);
        }
    }

    fn newline(&mut self, force: bool) {
        if force || !self.current.is_empty() {
            self.lines
                .push(Line::from(std::mem::take(&mut self.current)));
        }
    }
}

fn render(events: &[Event<'static>], code: &mut Vec<CodeCache>) -> Vec<Line<'static>> {
    let mut r = Renderer::default();
    let mut code_index = 0;
    let mut index = 0;
    while index < events.len() {
        let event = &events[index];
        index += 1;
        let style = r.style();
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                r.newline(false);
                let language = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_ascii_lowercase(),
                    CodeBlockKind::Indented => String::new(),
                };
                let mut source = String::new();
                while index < events.len() {
                    match &events[index] {
                        Event::Text(text) => source.push_str(text),
                        Event::End(TagEnd::CodeBlock) => {
                            index += 1;
                            break;
                        }
                        _ => {}
                    }
                    index += 1;
                }
                if code_index == code.len() {
                    code.push(CodeCache::default());
                }
                if !language.is_empty() {
                    r.text(&language, Style::default().fg(Color::DarkGray));
                    r.newline(false);
                }
                for mut line in code[code_index].update(&language, &source) {
                    let prefix = format!("{}  ", r.prefix());
                    line.spans.insert(0, Span::raw(prefix));
                    r.lines.push(line);
                }
                code_index += 1;
            }
            Event::Start(
                tag @ (Tag::Emphasis
                | Tag::Strong
                | Tag::Strikethrough
                | Tag::Heading { .. }
                | Tag::Link { .. }
                | Tag::Image { .. }
                | Tag::TableHead),
            ) => {
                let style = match tag {
                    Tag::Emphasis => style.add_modifier(Modifier::ITALIC),
                    Tag::Strong | Tag::TableHead => style.add_modifier(Modifier::BOLD),
                    Tag::Strikethrough => style.add_modifier(Modifier::CROSSED_OUT),
                    Tag::Heading { .. } => {
                        r.newline(false);
                        style.fg(Color::Cyan).add_modifier(Modifier::BOLD)
                    }
                    Tag::Link { .. } => style.fg(Color::Blue).add_modifier(Modifier::UNDERLINED),
                    Tag::Image { .. } => {
                        r.text("[image] ", style);
                        style
                    }
                    _ => style,
                };
                r.styles.push(style);
            }
            Event::End(
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Image,
            ) => {
                r.styles.pop();
            }
            Event::End(TagEnd::Link) => {
                let mut depth = 1usize;
                for previous in events[..index - 1].iter().rev() {
                    match previous {
                        Event::End(TagEnd::Link) => depth += 1,
                        Event::Start(Tag::Link { dest_url, .. }) => {
                            depth -= 1;
                            if depth == 0 {
                                r.text(&format!(" ({dest_url})"), style);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                r.styles.pop();
            }
            Event::Start(Tag::BlockQuote(_)) => {
                r.newline(false);
                r.containers.push(Container::Quote);
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                r.newline(false);
                r.containers.pop();
            }
            Event::Start(Tag::List(start)) => {
                r.newline(false);
                r.containers.push(Container::List(*start));
            }
            Event::End(TagEnd::List(_)) => {
                r.newline(false);
                r.containers.pop();
            }
            Event::Start(Tag::Item) => {
                r.newline(false);
                let marker = match r.containers.last_mut() {
                    Some(Container::List(Some(next))) => {
                        let marker = format!("{next}. ");
                        *next = next.saturating_add(1);
                        marker
                    }
                    _ => "- ".to_owned(),
                };
                r.containers.push(Container::Item {
                    marker,
                    first: true,
                });
            }
            Event::End(TagEnd::Item) => {
                r.newline(false);
                r.containers.pop();
            }
            Event::End(TagEnd::Heading(_)) => {
                r.styles.pop();
                r.newline(false);
            }
            Event::End(TagEnd::Paragraph | TagEnd::HtmlBlock | TagEnd::TableRow) => {
                r.newline(false)
            }
            Event::End(TagEnd::TableHead) => {
                r.styles.pop();
                r.newline(false);
            }
            Event::Start(Tag::TableCell) => r.text("│ ", style),
            Event::End(TagEnd::TableCell) => r.text(" ", style),
            Event::Text(text) => r.text(text, style),
            Event::Code(text) => r.text(text, style.fg(Color::Cyan)),
            Event::Html(text) | Event::InlineHtml(text) => {
                r.text(text.trim_end_matches('\n'), style.fg(Color::DarkGray))
            }
            Event::SoftBreak => r.text(" ", style),
            Event::HardBreak => r.newline(true),
            Event::Rule => {
                r.newline(false);
                r.text("────────", Style::default().fg(Color::DarkGray));
                r.newline(false);
            }
            Event::TaskListMarker(checked) => r.text(if *checked { "[x] " } else { "[ ] " }, style),
            Event::FootnoteReference(label) => r.text(&format!("[{label}]"), style),
            Event::InlineMath(text) | Event::DisplayMath(text) => r.text(text, style),
            _ => {}
        }
    }
    code.truncate(code_index);
    r.newline(false);
    r.lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(document: &mut Document, width: u16) -> Vec<Line<'static>> {
        (0..document.layout(width))
            .map(|row| document.row(row).unwrap().1.clone())
            .collect()
    }

    fn text(document: &mut Document) -> String {
        rows(document, 100)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn renders_markdown_styles_containers_and_literal_html() {
        let mut document = Document::default();
        document.update("# Title\n\n**bold** and *italic* with `code` and ~~removed~~.\n\n> quote\n\n3. first\n4. second\n   - nested\n\n- [x] done\n\n<a>literal</a>\n\n| a | b |\n| - | - |\n| c | d |\n");
        let output = text(&mut document);
        assert!(output.contains("Title"));
        assert!(!output.contains("**"));
        assert!(output.contains("│ quote"));
        assert!(output.contains("3. first\n4. second\n   - nested"));
        assert!(output.contains("- [x] done"));
        assert!(output.contains("<a>literal</a>"));
        assert!(output.contains("│ a │ b"));
        let spans: Vec<_> = document
            .blocks
            .iter()
            .flat_map(|block| &block.lines)
            .flat_map(|line| &line.text.spans)
            .collect();
        assert!(
            spans
                .iter()
                .any(|span| span.content == "bold"
                    && span.style.add_modifier.contains(Modifier::BOLD))
        );
        assert!(
            spans.iter().any(|span| span.content == "italic"
                && span.style.add_modifier.contains(Modifier::ITALIC))
        );
    }

    #[test]
    fn every_stream_prefix_matches_fresh_parsing_and_highlighting() {
        let sources = [
            "Heading\n=======\n\nRead [reference][id].\n\n| name | value |\n| --- | --- |\n| one | two |\n\n- first\n\n  continued\n\n```rust\n/* multi\nline */\nlet text = \"hello\";\n```\n\n[id]: https://example.com\n",
            "> - **nested**\n>\n>   ~~~python\n>   value = \"\"\"multi\n>   line\"\"\"\n>   ~~~\n",
            "```rust\r\nlet crab = \"🦀\\u{4e16}\";\r\n```\r\n\r\n![alt](image.png) and [**link**](https://example.com)",
        ];
        for source in sources {
            let mut incremental = Document::default();
            for end in source
                .char_indices()
                .map(|(index, _)| index)
                .chain([source.len()])
            {
                incremental.update(&source[..end]);
                let mut fresh = Document::default();
                fresh.update(&source[..end]);
                assert_eq!(
                    rows(&mut incremental, 37),
                    rows(&mut fresh, 37),
                    "prefix {end}: {source:?}"
                );
            }
        }
        let mut document = Document::default();
        document.update(sources[0]);
        let output = text(&mut document);
        assert!(output.contains("reference (https://example.com)"));
    }

    #[test]
    fn streaming_code_blocks_only_highlight_new_or_incomplete_lines() {
        let mut document = Document::default();
        document.update("```rust\n/* start\n");
        let first = document.blocks[0].code[0].highlighted;
        document.update("```rust\n/* start\ncomment");
        assert_eq!(document.blocks[0].code[0].highlighted, first + 1);
        document.update("```rust\n/* start\ncomment */\nlet value = 42;\n");
        assert_eq!(document.blocks[0].code[0].highlighted, first + 3);
    }

    #[test]
    fn idle_and_resize_reuse_parsing_and_code_highlights() {
        let source = "Unchanged paragraph.\n\n```rust\nlet value = 42;\n```\n\nTail";
        let mut document = Document::default();
        document.update(source);
        rows(&mut document, 40);
        let cached = document.blocks[0].lines[0].rows.as_ptr();
        let highlighted = document.blocks[1].code[0].highlighted;
        document.update(source);
        rows(&mut document, 40);
        assert_eq!(document.parses, 1);
        assert_eq!(document.blocks[0].lines[0].rows.as_ptr(), cached);
        document.update(&format!("{source} appended"));
        rows(&mut document, 40);
        assert_eq!(document.blocks[0].lines[0].rows.as_ptr(), cached);
        rows(&mut document, 15);
        assert_eq!(document.parses, 2);
        assert_eq!(document.blocks[1].code[0].highlighted, highlighted);
    }

    #[test]
    fn code_appends_preserve_complete_lines_and_rollback_partial_lines() {
        let mut code = CodeCache::default();
        code.update("rust", "/* open\n");
        let committed = code.highlighted;
        code.update("rust", "/* open\ncomment");
        assert_eq!(code.highlighted, committed + 1);
        let result = code.update("rust", "/* open\ncomment */\nlet value = 1;\n");
        assert_eq!(code.highlighted, committed + 3);
        assert_eq!(
            result,
            CodeCache::default().update("rust", "/* open\ncomment */\nlet value = 1;\n")
        );
        let changed = code.update("python", "print(42)\n");
        assert_eq!(
            changed,
            CodeCache::default().update("python", "print(42)\n")
        );
        assert!(
            changed
                .iter()
                .flat_map(|line| &line.spans)
                .any(|span| matches!(span.style.fg, Some(Color::Rgb(..))))
        );
    }

    #[test]
    fn code_fallback_preserves_text_and_does_not_reuse_invalid_state() {
        let mut code = CodeCache::default();
        let source = format!(
            "{}\nlet value = 42;\n",
            "x".repeat(MAX_HIGHLIGHT_LINE_BYTES + 1)
        );
        let lines = code.update("rust", &source);
        assert!(code.disabled);
        assert_eq!(
            lines
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
            source.trim_end_matches('\n')
        );
        assert!(
            lines[1]
                .spans
                .iter()
                .all(|span| span.style == Style::default())
        );
        let unknown = code.update("unknown-language", "plain\n");
        assert_eq!(unknown[0].to_string(), "plain");
    }

    #[test]
    fn unicode_wrapping_preserves_styles_text_and_anchor_positions() {
        let line = Line::from(vec![
            Span::styled("Hello ", Style::default().fg(Color::Cyan)),
            Span::raw("\u{4e16}\u{754c} 👩‍💻 e\u{301} end"),
        ]);
        for width in [1, 2, 7, 12, 100] {
            let wrapped = wrap(&line, width);
            assert!(wrapped.iter().all(|row| row.line.width() <= width as usize));
            if width >= 2 {
                assert_eq!(
                    wrapped
                        .iter()
                        .map(|row| row.line.to_string())
                        .collect::<String>(),
                    line.to_string()
                );
            }
        }
        let mut document = Document::default();
        document.update("Hello world with several words and more text");
        document.layout(10);
        let position = document.row(2).unwrap().0;
        document.layout(7);
        let located = document.locate(position);
        assert!(document.row(located).unwrap().0.byte <= position.byte);
        assert_eq!(document.layout(0), 0);
    }

    #[test]
    fn layout_anchors_round_trip_including_block_gaps() {
        let mut document = Document::default();
        document.update("First block\n\nSecond block with wrapping\n\n- one\n- two");
        let height = document.layout(12);
        for row in 0..height {
            assert_eq!(document.locate(document.row(row).unwrap().0), row);
        }
    }

    #[test]
    fn terminal_controls_are_escaped_after_markdown_parsing() {
        let mut document = Document::default();
        document.update("# Hello\n\n```text\n\u{1b}[2J\tvalue\n```\n\n[link](https://example.com)");
        let output = text(&mut document);
        assert!(!output.contains('\u{1b}'));
        assert!(output.contains("\\u{1b}[2J    value"));
        assert!(output.contains("link (https://example.com)"));
    }
}
