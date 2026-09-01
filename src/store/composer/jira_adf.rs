use serde_json::{Map, Value, json};

// Keep the MCP CHANGE_SET_GUIDANCE in mcp.rs and docs/jira-description-support.md updated with
// every ADF conversion, Jira-tag, validation, or overwrite-safety change. MCP agents rely on it
// before they read or write Composer descriptions.
pub(crate) fn adf_to_markdown(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Object(node) if node_type(node) == Some("doc") => render_blocks(content(node)),
        Value::Object(node) => render_block(node),
        Value::Array(nodes) => render_blocks(nodes),
        _ => String::new(),
    }
    .trim()
    .to_owned()
}

pub(crate) fn markdown_to_adf(markdown: &str) -> Value {
    json!({ "type": "doc", "version": 1, "content": parse_blocks(markdown) })
}

pub(crate) fn validate_markdown(markdown: &str) -> Result<(), String> {
    let mut open_panel = None;
    for (index, line) in markdown.lines().enumerate() {
        let line_number = index + 1;
        let trimmed = line.trim();
        if let Some(payload) = trimmed.strip_prefix("{{jira:panel ") {
            let valid = json_attrs(payload)
                .is_some_and(|(attrs, tail)| tail == "}}" && has_string_attr(&attrs, "panelType"));
            if !valid {
                return Err(format!("invalid Jira panel tag on line {line_number}"));
            }
            if open_panel.replace(line_number).is_some() {
                return Err(format!("nested Jira panel tag on line {line_number}"));
            }
            continue;
        }
        if trimmed.starts_with("{{/jira:panel") {
            if trimmed != "{{/jira:panel}}" {
                return Err(format!(
                    "invalid Jira panel closing tag on line {line_number}"
                ));
            }
            if open_panel.take().is_none() {
                return Err(format!(
                    "Jira panel closing tag without an opening tag on line {line_number}"
                ));
            }
            continue;
        }

        let mut remaining = line;
        while let Some(index) = find_unescaped(remaining, "{{jira:") {
            let tag = &remaining[index..];
            if tag.starts_with("{{jira:panel ") || tag.starts_with("{{/jira:panel}}") {
                return Err(format!(
                    "Jira panel tags must be on their own lines (line {line_number})"
                ));
            }
            let Some((node, rest)) = jira_inline_node(tag) else {
                return Err(format!("invalid Jira inline tag on line {line_number}"));
            };
            if !is_valid_jira_inline_node(&node) {
                return Err(format!("invalid Jira inline tag on line {line_number}"));
            }
            let consumed = tag.len() - rest.len();
            remaining = &tag[consumed..];
        }
    }
    if let Some(line_number) = open_panel {
        return Err(format!(
            "Jira panel opened on line {line_number} is not closed"
        ));
    }
    Ok(())
}

pub(crate) fn adf_is_safe_to_overwrite(value: &Value) -> bool {
    matches!(value, Value::Null | Value::String(_))
        || (value.is_object()
            && normalize_adf(markdown_to_adf(&adf_to_markdown(value)))
                == normalize_adf(value.clone()))
}

pub(crate) fn adf_overwrite_warning(value: &Value) -> Option<String> {
    (!adf_is_safe_to_overwrite(value)).then(|| {
        first_unsupported_feature(value)
            .unwrap_or_else(|| "formatting that Finery cannot preserve exactly".into())
    })
}

fn render_blocks(nodes: &[Value]) -> String {
    nodes
        .iter()
        .filter_map(Value::as_object)
        .map(render_block)
        .filter(|block| !block.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn render_block(node: &Map<String, Value>) -> String {
    match node_type(node).unwrap_or_default() {
        "paragraph" => render_inline(content(node)),
        "heading" => {
            let level = node
                .get("attrs")
                .and_then(|attrs| attrs.get("level"))
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .clamp(1, 6) as usize;
            format!("{} {}", "#".repeat(level), render_inline(content(node)))
        }
        "bulletList" => render_list(content(node), None),
        "orderedList" => {
            let start = node
                .get("attrs")
                .and_then(|attrs| attrs.get("order"))
                .and_then(Value::as_u64)
                .unwrap_or(1) as usize;
            render_list(content(node), Some(start))
        }
        "blockquote" => render_blocks(content(node))
            .lines()
            .map(|line| format!("> {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
        "codeBlock" => {
            let language = node
                .get("attrs")
                .and_then(|attrs| attrs.get("language"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            format!("```{language}\n{}\n```", plain_text(content(node)))
        }
        "rule" => "---".into(),
        "panel" => render_panel(node),
        "table" => render_table(node).unwrap_or_else(|| "<!-- unsupported Jira table -->".into()),
        "mediaSingle" | "mediaGroup" => "<!-- unsupported Jira media -->".into(),
        unknown => {
            let children = render_blocks(content(node));
            if children.is_empty() && !unknown.is_empty() {
                format!("<!-- unsupported ADF node: {unknown} -->")
            } else {
                children
            }
        }
    }
}

fn render_panel(node: &Map<String, Value>) -> String {
    let attrs = node
        .get("attrs")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    format!(
        "{{{{jira:panel {}}}}}\n{}\n{{{{/jira:panel}}}}",
        Value::Object(attrs),
        render_blocks(content(node))
    )
}

fn render_table(node: &Map<String, Value>) -> Option<String> {
    let rows = content(node);
    let header = table_row(rows.first()?.as_object()?, "tableHeader")?;
    let width = header.len();
    let mut lines = vec![format_table_row(&header), format_table_divider(width)];
    for row in &rows[1..] {
        let cells = table_row(row.as_object()?, "tableCell")?;
        (cells.len() == width).then_some(())?;
        lines.push(format_table_row(&cells));
    }
    Some(lines.join("\n"))
}

fn table_row(node: &Map<String, Value>, cell_type: &str) -> Option<Vec<String>> {
    (node_type(node) == Some("tableRow")).then_some(())?;
    content(node)
        .iter()
        .map(Value::as_object)
        .map(|cell| table_cell(cell?, cell_type))
        .collect()
}

fn table_cell(node: &Map<String, Value>, cell_type: &str) -> Option<String> {
    (node_type(node) == Some(cell_type)).then_some(())?;
    let paragraph = content(node).first()?.as_object()?;
    (content(node).len() == 1 && node_type(paragraph) == Some("paragraph")).then_some(())?;
    (!content(paragraph).iter().any(|child| {
        child
            .as_object()
            .is_some_and(|node| node_type(node) == Some("hardBreak"))
    }))
    .then_some(())?;
    Some(render_inline(content(paragraph)))
}

fn format_table_row(cells: &[String]) -> String {
    format!("| {} |", cells.join(" | "))
}

fn format_table_divider(width: usize) -> String {
    format!("| {} |", vec!["---"; width].join(" | "))
}

fn render_list(items: &[Value], ordered_start: Option<usize>) -> String {
    items
        .iter()
        .filter_map(Value::as_object)
        .enumerate()
        .map(|(index, item)| {
            let marker = ordered_start
                .map(|start| format!("{}. ", start + index))
                .unwrap_or_else(|| "- ".into());
            let body = render_list_item(content(item));
            let continuation = " ".repeat(marker.chars().count());
            body.lines()
                .enumerate()
                .map(|(line_index, line)| {
                    if line_index == 0 {
                        format!("{marker}{line}")
                    } else {
                        format!("{continuation}{line}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_list_item(nodes: &[Value]) -> String {
    let mut output = String::new();
    let mut previous_was_list = false;
    for node in nodes.iter().filter_map(Value::as_object) {
        let block = render_block(node);
        if block.is_empty() {
            continue;
        }
        let is_list = matches!(node_type(node), Some("bulletList" | "orderedList"));
        if !output.is_empty() {
            output.push_str(if is_list || previous_was_list {
                "\n"
            } else {
                "\n\n"
            });
        }
        output.push_str(&block);
        previous_was_list = is_list;
    }
    output
}

fn render_inline(nodes: &[Value]) -> String {
    nodes
        .iter()
        .filter_map(Value::as_object)
        .map(|node| match node_type(node).unwrap_or_default() {
            "text" => render_text(node),
            "hardBreak" => "  \n".into(),
            "mention" => render_inline_node("jira:mention", node),
            "emoji" => node
                .get("attrs")
                .and_then(|attrs| attrs.get("text").or_else(|| attrs.get("shortName")))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            "inlineCard" => render_inline_node("jira:inline-card", node),
            _ => render_inline(content(node)),
        })
        .collect()
}

fn render_inline_node(kind: &str, node: &Map<String, Value>) -> String {
    let attrs = node
        .get("attrs")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    format!("{{{{{kind} {} /}}}}", Value::Object(attrs))
}

fn render_text(node: &Map<String, Value>) -> String {
    let raw = node.get("text").and_then(Value::as_str).unwrap_or_default();
    let marks = node
        .get("marks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .collect::<Vec<_>>();
    let is_code = marks.iter().any(|mark| node_type(mark) == Some("code"));
    let mut text = if is_code {
        raw.to_owned()
    } else {
        escape_markdown(raw)
    };
    for mark in marks {
        text = match node_type(mark).unwrap_or_default() {
            "strong" => format!("**{text}**"),
            "em" => format!("*{text}*"),
            "strike" => format!("~~{text}~~"),
            "code" => format!("`{text}`"),
            "link" => format!(
                "[{text}]({})",
                mark.get("attrs")
                    .and_then(|attrs| attrs.get("href"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            ),
            _ => text,
        };
    }
    text
}

fn plain_text(nodes: &[Value]) -> String {
    nodes
        .iter()
        .filter_map(Value::as_object)
        .map(|node| {
            node.get("text")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| plain_text(content(node)))
        })
        .collect()
}

fn first_unsupported_feature(value: &Value) -> Option<String> {
    let node = value.as_object()?;
    match node_type(node)? {
        "panel" => {}
        "table" if render_table(node).is_some() => {}
        "table" => return Some("a Jira table structure Finery cannot preserve exactly".into()),
        "mediaSingle" | "mediaGroup" => return Some("Jira media".into()),
        "mention" => {}
        "emoji" => return Some("Jira emoji".into()),
        "inlineCard" => {}
        "text" => {
            for mark in node
                .get("marks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_object)
            {
                match node_type(mark).unwrap_or_default() {
                    "strong" | "em" | "strike" | "code" | "link" => {}
                    "underline" => return Some("underlined text".into()),
                    "textColor" => return Some("text colour".into()),
                    "backgroundColor" => return Some("text background colour".into()),
                    mark => return Some(format!("Jira {mark} text formatting")),
                }
            }
        }
        "doc" | "paragraph" | "heading" | "bulletList" | "orderedList" | "listItem"
        | "blockquote" | "codeBlock" | "rule" | "hardBreak" | "tableRow" | "tableHeader"
        | "tableCell" => {}
        node_type => return Some(format!("Jira {node_type} content")),
    }
    content(node).iter().find_map(first_unsupported_feature)
}

fn escape_markdown(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('*', "\\*")
        .replace('_', "\\_")
        .replace('`', "\\`")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('|', "\\|")
        .replace("{{", "\\{\\{")
        .replace('~', "\\~")
}

fn parse_blocks(markdown: &str) -> Vec<Value> {
    let lines = markdown.lines().collect::<Vec<_>>();
    let mut blocks = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if line.trim().is_empty() {
            index += 1;
            continue;
        }
        if let Some((attrs, next)) = panel_start(&lines, index) {
            let body = lines[index + 1..next].join("\n");
            let mut panel = json!({ "type": "panel", "content": parse_blocks(&body) });
            if !attrs.is_empty() {
                panel["attrs"] = Value::Object(attrs);
            }
            blocks.push(panel);
            index = next + 1;
            continue;
        }
        if let Some(language) = line.trim().strip_prefix("```") {
            index += 1;
            let mut code = Vec::new();
            while index < lines.len() && !lines[index].trim().starts_with("```") {
                code.push(lines[index]);
                index += 1;
            }
            if index < lines.len() {
                index += 1;
            }
            blocks.push(code_block(language.trim(), &code.join("\n")));
            continue;
        }
        if let Some((level, text)) = markdown_heading(line) {
            blocks.push(json!({
                "type": "heading",
                "attrs": { "level": level },
                "content": parse_inline(text),
            }));
            index += 1;
            continue;
        }
        if line.trim() == "---" {
            blocks.push(json!({ "type": "rule" }));
            index += 1;
            continue;
        }
        if let Some((table, next)) = markdown_table(&lines, index) {
            blocks.push(table);
            index = next;
            continue;
        }
        if let Some(item) = list_item(line) {
            let (list, next) = parse_list(&lines, index, item.indent, item.ordered);
            blocks.push(list);
            index = next;
            continue;
        }
        if line.trim_start().starts_with("> ") {
            let mut quote = Vec::new();
            while index < lines.len() {
                let Some(value) = lines[index].trim_start().strip_prefix("> ") else {
                    break;
                };
                quote.push(value);
                index += 1;
            }
            blocks.push(json!({
                "type": "blockquote",
                "content": parse_blocks(&quote.join("\n")),
            }));
            continue;
        }
        let mut paragraph = vec![line];
        index += 1;
        while index < lines.len() && !lines[index].trim().is_empty() && !starts_block(lines[index])
        {
            paragraph.push(lines[index]);
            index += 1;
        }
        blocks.push(json!({
            "type": "paragraph",
            "content": parse_inline(&paragraph.join("\n")),
        }));
    }
    blocks
}

fn panel_start(lines: &[&str], start: usize) -> Option<(Map<String, Value>, usize)> {
    let line = lines.get(start)?.trim();
    let payload = line.strip_prefix("{{jira:panel ")?;
    let (attrs, tail) = json_attrs(payload)?;
    (tail == "}}").then_some(())?;
    let end = lines[start + 1..]
        .iter()
        .position(|line| line.trim() == "{{/jira:panel}}")?;
    Some((attrs, start + end + 1))
}

fn markdown_table(lines: &[&str], start: usize) -> Option<(Value, usize)> {
    let header = table_cells(lines.get(start)?)?;
    let divider = table_cells(lines.get(start + 1)?)?;
    (header.len() == divider.len() && divider.iter().all(|cell| is_table_divider(cell)))
        .then_some(())?;
    let mut rows = vec![adf_table_row("tableHeader", &header)];
    let mut index = start + 2;
    while let Some(row) = lines.get(index).and_then(|line| table_cells(line)) {
        if row.len() != header.len() {
            break;
        }
        rows.push(adf_table_row("tableCell", &row));
        index += 1;
    }
    Some((json!({ "type": "table", "content": rows }), index))
}

fn table_cells(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    (trimmed.starts_with('|') && trimmed.ends_with('|')).then_some(())?;
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut escaped = false;
    for character in trimmed[1..trimmed.len() - 1].chars() {
        if escaped {
            cell.push('\\');
            cell.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '|' {
            cells.push(cell.trim().to_owned());
            cell.clear();
        } else {
            cell.push(character);
        }
    }
    if escaped {
        cell.push('\\');
    }
    cells.push(cell.trim().to_owned());
    (!cells.is_empty()).then_some(cells)
}

fn is_table_divider(cell: &str) -> bool {
    let cell = cell.trim_matches(':');
    cell.len() >= 3 && cell.chars().all(|character| character == '-')
}

fn adf_table_row(cell_type: &str, cells: &[String]) -> Value {
    json!({
        "type": "tableRow",
        "content": cells.iter().map(|cell| json!({
            "type": cell_type,
            "content": [{
                "type": "paragraph",
                "content": parse_inline(cell),
            }],
        })).collect::<Vec<_>>(),
    })
}

fn markdown_heading(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let hashes = trimmed
        .chars()
        .take_while(|character| *character == '#')
        .count();
    (1..=6).contains(&hashes).then(|| {
        trimmed
            .get(hashes..)?
            .strip_prefix(' ')
            .map(|text| (hashes, text))
    })?
}

fn starts_block(line: &str) -> bool {
    markdown_heading(line).is_some()
        || list_item(line).is_some()
        || line.trim().starts_with("{{jira:panel ")
        || table_cells(line).is_some()
        || line.trim_start().starts_with("> ")
        || line.trim().starts_with("```")
        || line.trim() == "---"
}

fn unordered_item(line: &str) -> Option<&str> {
    ["- ", "* ", "+ "]
        .into_iter()
        .find_map(|prefix| line.strip_prefix(prefix))
}

fn ordered_item(line: &str) -> Option<(usize, &str)> {
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    let number = line.get(..digits)?.parse().ok()?;
    let text = line.get(digits..)?.strip_prefix(". ")?;
    Some((number, text))
}

struct MarkdownListItem<'a> {
    indent: usize,
    ordered: bool,
    number: Option<usize>,
    text: &'a str,
}

fn list_item(line: &str) -> Option<MarkdownListItem<'_>> {
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let text = &line[indent..];
    if let Some(text) = unordered_item(text) {
        return Some(MarkdownListItem {
            indent,
            ordered: false,
            number: None,
            text,
        });
    }
    let (number, text) = ordered_item(text)?;
    Some(MarkdownListItem {
        indent,
        ordered: true,
        number: Some(number),
        text,
    })
}

fn parse_list(lines: &[&str], start: usize, indent: usize, ordered: bool) -> (Value, usize) {
    let mut index = start;
    let mut items = Vec::new();
    let mut order = 1;
    while index < lines.len() {
        let Some(item) = list_item(lines[index]) else {
            break;
        };
        if item.indent != indent || item.ordered != ordered {
            break;
        }
        if ordered {
            let number = item.number.expect("ordered list item has a number");
            if items.is_empty() {
                order = number;
            }
        }
        items.push(json!({
            "type": "listItem",
            "content": [{
                "type": "paragraph",
                "content": parse_inline(item.text),
            }],
        }));
        index += 1;
        while let Some(child) = lines.get(index).and_then(|line| list_item(line)) {
            if child.indent <= indent {
                break;
            }
            let (nested, next) = parse_list(lines, index, child.indent, child.ordered);
            items
                .last_mut()
                .and_then(|value| value.get_mut("content"))
                .and_then(Value::as_array_mut)
                .expect("list item content exists")
                .push(nested);
            index = next;
        }
    }
    let list = if ordered {
        json!({ "type": "orderedList", "attrs": { "order": order }, "content": items })
    } else {
        json!({ "type": "bulletList", "content": items })
    };
    (list, index)
}

fn parse_inline(text: &str) -> Vec<Value> {
    let mut nodes = Vec::new();
    let mut remaining = text;
    while !remaining.is_empty() {
        if let Some(rest) = remaining.strip_prefix('\\') {
            let Some(character) = rest.chars().next() else {
                push_text(&mut nodes, "\\".into(), Vec::new());
                break;
            };
            push_text(&mut nodes, character.to_string(), Vec::new());
            remaining = &rest[character.len_utf8()..];
            continue;
        }
        if let Some(rest) = remaining.strip_prefix("  \n") {
            nodes.push(json!({ "type": "hardBreak" }));
            remaining = rest;
            continue;
        }
        if let Some(rest) = remaining.strip_prefix('\n') {
            nodes.push(json!({ "type": "hardBreak" }));
            remaining = rest;
            continue;
        }
        if let Some((node, rest)) = jira_inline_node(remaining) {
            nodes.push(node);
            remaining = rest;
            continue;
        }
        if let Some((node, rest)) = marked_inline(remaining, "**", "strong")
            .or_else(|| marked_inline(remaining, "~~", "strike"))
            .or_else(|| marked_inline(remaining, "`", "code"))
            .or_else(|| marked_inline(remaining, "*", "em"))
        {
            nodes.push(node);
            remaining = rest;
            continue;
        }
        if let Some((node, rest)) = markdown_link(remaining) {
            nodes.push(node);
            remaining = rest;
            continue;
        }
        let next = next_inline_marker(remaining).unwrap_or(remaining.len());
        let take = next.max(remaining.chars().next().map(char::len_utf8).unwrap_or(0));
        let (text, rest) = remaining.split_at(take);
        push_text(&mut nodes, unescape_markdown(text), Vec::new());
        remaining = rest;
    }
    nodes
}

fn marked_inline<'a>(source: &'a str, delimiter: &str, mark: &str) -> Option<(Value, &'a str)> {
    let rest = source.strip_prefix(delimiter)?;
    let end = find_unescaped(rest, delimiter)?;
    let text = &rest[..end];
    let remaining = &rest[end + delimiter.len()..];
    Some((
        json!({
            "type": "text",
            "text": unescape_markdown(text),
            "marks": [{ "type": mark }],
        }),
        remaining,
    ))
}

fn markdown_link(source: &str) -> Option<(Value, &str)> {
    let label = source.strip_prefix('[')?;
    let label_end = find_unescaped(label, "](")?;
    let url = &label[label_end + 2..];
    let url_end = find_unescaped(url, ")")?;
    Some((
        json!({
            "type": "text",
            "text": unescape_markdown(&label[..label_end]),
            "marks": [{ "type": "link", "attrs": { "href": &url[..url_end] } }],
        }),
        &url[url_end + 1..],
    ))
}

fn jira_inline_node(source: &str) -> Option<(Value, &str)> {
    for (token, node_type) in [
        ("{{jira:mention ", "mention"),
        ("{{jira:inline-card ", "inlineCard"),
    ] {
        let Some(payload) = source.strip_prefix(token) else {
            continue;
        };
        let (attrs, rest) = json_attrs(payload)?;
        let rest = rest.strip_prefix(" /}}")?;
        return Some((json!({ "type": node_type, "attrs": attrs }), rest));
    }
    None
}

fn is_valid_jira_inline_node(node: &Value) -> bool {
    let Some(attrs) = node.get("attrs").and_then(Value::as_object) else {
        return false;
    };
    match node.get("type").and_then(Value::as_str) {
        Some("mention") => has_string_attr(attrs, "id") && has_string_attr(attrs, "text"),
        Some("inlineCard") => has_string_attr(attrs, "url"),
        _ => false,
    }
}

fn has_string_attr(attrs: &Map<String, Value>, key: &str) -> bool {
    attrs
        .get(key)
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
}

fn json_attrs(source: &str) -> Option<(Map<String, Value>, &str)> {
    let mut values = serde_json::Deserializer::from_str(source).into_iter::<Value>();
    let attrs = values.next()?.ok()?.as_object()?.clone();
    Some((attrs, &source[values.byte_offset()..]))
}

fn next_inline_marker(source: &str) -> Option<usize> {
    ["{{", "**", "~~", "`", "*", "[", "\n"]
        .into_iter()
        .filter_map(|marker| find_unescaped(source, marker))
        .min()
}

fn push_text(nodes: &mut Vec<Value>, text: String, marks: Vec<Value>) {
    if text.is_empty() {
        return;
    }
    if let Some(previous) = nodes.last_mut()
        && previous.get("type") == Some(&Value::String("text".into()))
        && previous.get("marks") == (!marks.is_empty()).then_some(&Value::Array(marks.clone()))
    {
        previous["text"] = Value::String(format!(
            "{}{}",
            previous["text"].as_str().unwrap_or_default(),
            text
        ));
        return;
    }
    let mut node = json!({ "type": "text", "text": text });
    if !marks.is_empty() {
        node["marks"] = Value::Array(marks);
    }
    nodes.push(node);
}

fn unescape_markdown(text: &str) -> String {
    let mut output = String::new();
    let mut escaped = false;
    for character in text.chars() {
        if escaped {
            output.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else {
            output.push(character);
        }
    }
    if escaped {
        output.push('\\');
    }
    output
}

fn find_unescaped(source: &str, marker: &str) -> Option<usize> {
    source.match_indices(marker).find_map(|(index, _)| {
        let escaped = source[..index]
            .chars()
            .rev()
            .take_while(|character| *character == '\\')
            .count()
            % 2
            == 1;
        (!escaped).then_some(index)
    })
}

fn normalize_adf(mut value: Value) -> Value {
    normalize_adf_node(&mut value);
    value
}

fn normalize_adf_node(value: &mut Value) {
    let Some(node) = value.as_object_mut() else {
        return;
    };
    if node
        .get("attrs")
        .and_then(Value::as_object)
        .is_some_and(Map::is_empty)
    {
        node.remove("attrs");
    }
    let Some(children) = node.get_mut("content").and_then(Value::as_array_mut) else {
        return;
    };
    for child in children.iter_mut() {
        normalize_adf_node(child);
    }
    let mut normalized = Vec::with_capacity(children.len());
    for child in std::mem::take(children) {
        let can_merge = normalized.last().is_some_and(|previous: &Value| {
            previous.get("type") == Some(&Value::String("text".into()))
                && child.get("type") == Some(&Value::String("text".into()))
                && previous.get("marks") == child.get("marks")
        });
        if can_merge {
            let previous = normalized.last_mut().expect("text node exists");
            previous["text"] = Value::String(format!(
                "{}{}",
                previous["text"].as_str().unwrap_or_default(),
                child["text"].as_str().unwrap_or_default()
            ));
        } else {
            normalized.push(child);
        }
    }
    *children = normalized;
}

fn node_type(node: &Map<String, Value>) -> Option<&str> {
    node.get("type").and_then(Value::as_str)
}

fn content(node: &Map<String, Value>) -> &[Value] {
    node.get("content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn code_block(language: &str, code: &str) -> Value {
    let mut node = json!({
        "type": "codeBlock",
        "content": [{ "type": "text", "text": code }],
    });
    if !language.is_empty() {
        node["attrs"] = json!({ "language": language });
    }
    node
}
