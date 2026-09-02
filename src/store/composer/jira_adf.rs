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
    let mut document = json!({ "type": "doc", "version": 1, "content": parse_blocks(markdown) });
    assign_list_local_ids(&mut document, &mut 1);
    document
}

pub(crate) fn validate_markdown(markdown: &str) -> Result<(), String> {
    let mut open_block = None;
    for (index, line) in markdown.lines().enumerate() {
        let line_number = index + 1;
        let trimmed = line.trim();
        if let Some(payload) = trimmed.strip_prefix("{{jira:panel ") {
            let valid = json_attrs(payload)
                .is_some_and(|(attrs, tail)| tail == "}}" && has_string_attr(&attrs, "panelType"));
            if !valid {
                return Err(format!("invalid Jira panel tag on line {line_number}"));
            }
            if open_block.replace(("panel", line_number)).is_some() {
                return Err(format!("nested Jira block tag on line {line_number}"));
            }
            continue;
        }
        if let Some(kind) = jira_block_start(trimmed) {
            if open_block.replace((kind, line_number)).is_some() {
                return Err(format!("nested Jira block tag on line {line_number}"));
            }
            continue;
        }
        if let Some(kind) = jira_block_end(trimmed) {
            let Some((open_kind, _)) = open_block.take() else {
                return Err(format!(
                    "Jira {kind} closing tag without an opening tag on line {line_number}"
                ));
            };
            if open_kind != kind {
                return Err(format!(
                    "mismatched Jira block closing tag on line {line_number}"
                ));
            }
            continue;
        }
        if trimmed.starts_with("{{jira:mention") || trimmed.starts_with("{{jira:inline-card") {
            validate_inline(line, line_number)?;
            continue;
        }
        if trimmed.starts_with("{{jira:") || trimmed.starts_with("{{/jira:") {
            return Err(format!("invalid Jira block tag on line {line_number}"));
        }
        if let Some((kind, _)) = open_block {
            match kind {
                "task-list" if task_list_item(trimmed).is_none() => {
                    return Err(format!("invalid Jira task list item on line {line_number}"));
                }
                "decision-list" if decision_list_item(trimmed).is_none() => {
                    return Err(format!(
                        "invalid Jira decision list item on line {line_number}"
                    ));
                }
                "panel" => validate_inline(line, line_number)?,
                _ => {}
            }
            if let Some(text) = task_list_item(trimmed)
                .map(|(_, text)| text)
                .or_else(|| decision_list_item(trimmed))
            {
                validate_inline(text, line_number)?;
            }
        } else {
            validate_inline(line, line_number)?;
        }
    }
    if let Some((kind, line_number)) = open_block {
        return Err(format!(
            "Jira {kind} opened on line {line_number} is not closed"
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
        "taskList" => render_task_list(node),
        "decisionList" => render_decision_list(node),
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
            "mention" => render_mention(node),
            "emoji" => node
                .get("attrs")
                .and_then(|attrs| attrs.get("shortName"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            "date" => render_date(node),
            "status" => render_status(node),
            "inlineCard" => render_card(node),
            _ => render_inline(content(node)),
        })
        .collect()
}

fn render_task_list(node: &Map<String, Value>) -> String {
    let items = content(node)
        .iter()
        .filter_map(Value::as_object)
        .map(|item| {
            let done = item
                .get("attrs")
                .and_then(|attrs| attrs.get("state"))
                .and_then(Value::as_str)
                == Some("DONE");
            format!(
                "- [{}] {}",
                if done { "x" } else { " " },
                render_inline(content(item))
            )
        })
        .collect::<Vec<_>>();
    format!(
        "{{{{jira:task-list}}}}\n{}\n{{{{/jira:task-list}}}}",
        items.join("\n")
    )
}

fn render_decision_list(node: &Map<String, Value>) -> String {
    let items = content(node)
        .iter()
        .filter_map(Value::as_object)
        .map(|item| format!("- {}", render_inline(content(item))))
        .collect::<Vec<_>>();
    format!(
        "{{{{jira:decision-list}}}}\n{}\n{{{{/jira:decision-list}}}}",
        items.join("\n")
    )
}

fn render_mention(node: &Map<String, Value>) -> String {
    let attrs = node.get("attrs").and_then(Value::as_object);
    let text = attrs
        .and_then(|attrs| attrs.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let id = attrs
        .and_then(|attrs| attrs.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    format!("@mention({}, {})", json!(text), json!(id))
}

fn render_date(node: &Map<String, Value>) -> String {
    let timestamp = node
        .get("attrs")
        .and_then(|attrs| attrs.get("timestamp"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    date_from_timestamp(timestamp)
        .map(|date| format!("@date({date})"))
        .unwrap_or_else(|| "<!-- unsupported Jira date -->".into())
}

fn render_status(node: &Map<String, Value>) -> String {
    let attrs = node.get("attrs").and_then(Value::as_object);
    let text = attrs
        .and_then(|attrs| attrs.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let color = attrs
        .and_then(|attrs| attrs.get("color"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    format!("@status({}, {color})", json!(text))
}

fn render_card(node: &Map<String, Value>) -> String {
    let url = node
        .get("attrs")
        .and_then(|attrs| attrs.get("url"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    format!("@card({url})")
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
            "underline" => format!("++{text}++"),
            "textColor" => mark_color(&text, mark, "color"),
            "backgroundColor" => text,
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

fn mark_color(text: &str, mark: &Map<String, Value>, name: &str) -> String {
    let color = mark
        .get("attrs")
        .and_then(|attrs| attrs.get("color"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    format!("{{{name}:{color}}}{text}{{/{name}}}")
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
        "taskList" if valid_task_list(node) => {}
        "taskList" => return Some("a Jira task list Finery cannot preserve exactly".into()),
        "decisionList" if valid_decision_list(node) => {}
        "decisionList" => {
            return Some("a Jira decision list Finery cannot preserve exactly".into());
        }
        "table" if render_table(node).is_some() => {}
        "table" => return Some("a Jira table structure Finery cannot preserve exactly".into()),
        "mediaSingle" | "mediaGroup" => return Some("Jira media".into()),
        "mention" if valid_mention(node) => {}
        "mention" => return Some("a Jira mention Finery cannot preserve exactly".into()),
        "emoji" if valid_emoji(node) => {}
        "emoji" => return Some("a Jira emoji Finery cannot preserve exactly".into()),
        "date" if valid_date(node) => {}
        "date" => return Some("a Jira date with a time of day or invalid timestamp".into()),
        "status" if valid_status(node) => {}
        "status" => return Some("a Jira status Finery cannot preserve exactly".into()),
        "inlineCard" if valid_card(node) => {}
        "inlineCard" => return Some("a Jira smart link Finery cannot preserve exactly".into()),
        "text" => {
            for mark in node
                .get("marks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_object)
            {
                match node_type(mark).unwrap_or_default() {
                    "strong" | "em" | "strike" | "code" | "link" | "underline" => {}
                    "textColor" if valid_color_mark(mark) => {}
                    "textColor" => {
                        return Some("text colour Finery cannot preserve exactly".into());
                    }
                    "backgroundColor" => {
                        return Some(
                            "text background colour Finery cannot preserve exactly".into(),
                        );
                    }
                    mark => return Some(format!("Jira {mark} text formatting")),
                }
            }
        }
        "doc" | "paragraph" | "heading" | "bulletList" | "orderedList" | "listItem"
        | "blockquote" | "codeBlock" | "rule" | "hardBreak" | "tableRow" | "tableHeader"
        | "tableCell" | "taskItem" | "decisionItem" => {}
        node_type => return Some(format!("Jira {node_type} content")),
    }
    content(node).iter().find_map(first_unsupported_feature)
}

fn escape_markdown(text: &str) -> String {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('*', "\\*")
        .replace('_', "\\_")
        .replace('`', "\\`")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('|', "\\|")
        .replace("{{", "\\{\\{")
        .replace('~', "\\~");
    escape_canonical_literals(&escaped)
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
        if let Some((list, next)) = jira_task_list(&lines, index) {
            blocks.push(list);
            index = next;
            continue;
        }
        if let Some((list, next)) = jira_decision_list(&lines, index) {
            blocks.push(list);
            index = next;
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

fn jira_task_list(lines: &[&str], start: usize) -> Option<(Value, usize)> {
    (lines.get(start)?.trim() == "{{jira:task-list}}").then_some(())?;
    let end = lines[start + 1..]
        .iter()
        .position(|line| line.trim() == "{{/jira:task-list}}")?;
    let items = lines[start + 1..start + end + 1]
        .iter()
        .map(|line| task_list_item(line.trim()))
        .collect::<Option<Vec<_>>>()?;
    Some((
        json!({
            "type": "taskList",
            "attrs": { "localId": "00000000-0000-4000-8000-000000000001" },
            "content": items.into_iter().enumerate().map(|(index, (state, text))| json!({
                "type": "taskItem",
                "attrs": { "state": state, "localId": local_id(2 + index) },
                "content": parse_inline(text),
            })).collect::<Vec<_>>(),
        }),
        start + end + 2,
    ))
}

fn jira_decision_list(lines: &[&str], start: usize) -> Option<(Value, usize)> {
    (lines.get(start)?.trim() == "{{jira:decision-list}}").then_some(())?;
    let end = lines[start + 1..]
        .iter()
        .position(|line| line.trim() == "{{/jira:decision-list}}")?;
    let items = lines[start + 1..start + end + 1]
        .iter()
        .map(|line| decision_list_item(line.trim()))
        .collect::<Option<Vec<_>>>()?;
    Some((
        json!({
            "type": "decisionList",
            "attrs": { "localId": "00000000-0000-4000-8000-000000000003" },
            "content": items.into_iter().enumerate().map(|(index, text)| json!({
                "type": "decisionItem",
                "attrs": { "state": "DECIDED", "localId": local_id(1_002 + index) },
                "content": parse_inline(text),
            })).collect::<Vec<_>>(),
        }),
        start + end + 2,
    ))
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
        || jira_block_start(line.trim()).is_some()
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
        if let Some((nodes_with_mark, rest)) = color_inline_nodes(remaining, "color", "textColor") {
            nodes.extend(nodes_with_mark);
            remaining = rest;
            continue;
        }
        if let Some((node, rest)) = canonical_inline_node(remaining) {
            nodes.push(node);
            remaining = rest;
            continue;
        }
        if let Some((nodes_with_mark, rest)) = code_inline_nodes(remaining)
            .or_else(|| marked_inline_nodes(remaining, "**", "strong"))
            .or_else(|| marked_inline_nodes(remaining, "~~", "strike"))
            .or_else(|| marked_inline_nodes(remaining, "++", "underline"))
            .or_else(|| marked_inline_nodes(remaining, "*", "em"))
        {
            nodes.extend(nodes_with_mark);
            remaining = rest;
            continue;
        }
        if let Some((nodes_with_mark, rest)) = markdown_link(remaining) {
            nodes.extend(nodes_with_mark);
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

fn code_inline_nodes(source: &str) -> Option<(Vec<Value>, &str)> {
    let text = source.strip_prefix('`')?;
    let end = find_unescaped(text, "`")?;
    let mut nodes = Vec::new();
    push_text(
        &mut nodes,
        text[..end].into(),
        vec![json!({ "type": "code" })],
    );
    Some((nodes, &text[end + 1..]))
}

fn marked_inline_nodes<'a>(
    source: &'a str,
    delimiter: &str,
    mark: &str,
) -> Option<(Vec<Value>, &'a str)> {
    let rest = source.strip_prefix(delimiter)?;
    let end = find_unescaped(rest, delimiter)?;
    let text = &rest[..end];
    let remaining = &rest[end + delimiter.len()..];
    let mut nodes = parse_inline(text);
    add_mark(&mut nodes, json!({ "type": mark }));
    Some((nodes, remaining))
}

fn markdown_link(source: &str) -> Option<(Vec<Value>, &str)> {
    let label = source.strip_prefix('[')?;
    let label_end = find_unescaped(label, "](")?;
    let url = &label[label_end + 2..];
    let url_end = find_unescaped(url, ")")?;
    let mut nodes = parse_inline(&label[..label_end]);
    add_mark(
        &mut nodes,
        json!({ "type": "link", "attrs": { "href": &url[..url_end] } }),
    );
    Some((nodes, &url[url_end + 1..]))
}

fn canonical_inline_node(source: &str) -> Option<(Value, &str)> {
    date_inline_node(source)
        .or_else(|| status_inline_node(source))
        .or_else(|| mention_inline_node(source))
        .or_else(|| card_inline_node(source))
        .or_else(|| emoji_inline_node(source))
}

fn date_inline_node(source: &str) -> Option<(Value, &str)> {
    let date = source.strip_prefix("@date(")?;
    let end = date.find(')')?;
    let timestamp = date_timestamp(&date[..end])?;
    Some((
        json!({ "type": "date", "attrs": { "timestamp": timestamp } }),
        &date[end + 1..],
    ))
}

fn status_inline_node(source: &str) -> Option<(Value, &str)> {
    let rest = source.strip_prefix("@status(")?;
    let (text, rest) = json_string(rest)?;
    let rest = rest.strip_prefix(", ")?;
    let color_end = rest.find(')')?;
    let color = &rest[..color_end];
    valid_status_color(color).then_some(())?;
    Some((
        json!({ "type": "status", "attrs": { "text": text, "color": color } }),
        &rest[color_end + 1..],
    ))
}

fn mention_inline_node(source: &str) -> Option<(Value, &str)> {
    let rest = source.strip_prefix("@mention(")?;
    let (text, rest) = json_string(rest)?;
    let rest = rest.strip_prefix(", ")?;
    let (id, rest) = json_string(rest)?;
    let rest = rest.strip_prefix(')')?;
    (!text.is_empty() && !id.is_empty()).then_some(())?;
    Some((
        json!({ "type": "mention", "attrs": { "text": text, "id": id } }),
        rest,
    ))
}

fn card_inline_node(source: &str) -> Option<(Value, &str)> {
    let rest = source.strip_prefix("@card(")?;
    let end = rest.find(')')?;
    let url = &rest[..end];
    valid_card_url(url).then_some(())?;
    Some((
        json!({ "type": "inlineCard", "attrs": { "url": url } }),
        &rest[end + 1..],
    ))
}

fn emoji_inline_node(source: &str) -> Option<(Value, &str)> {
    let name = source.strip_prefix(':')?;
    let end = name.find(':')?;
    let short_name = &name[..end];
    valid_emoji_short_name(short_name).then_some(())?;
    Some((
        json!({ "type": "emoji", "attrs": { "shortName": format!(":{short_name}:") } }),
        &name[end + 1..],
    ))
}

fn color_inline_nodes<'a>(
    source: &'a str,
    name: &str,
    mark_type: &str,
) -> Option<(Vec<Value>, &'a str)> {
    let rest = source.strip_prefix(&format!("{{{name}:"))?;
    let color_end = rest.find('}')?;
    let color = &rest[..color_end];
    valid_hex_color(color).then_some(())?;
    let text = &rest[color_end + 1..];
    let closing = format!("{{/{name}}}");
    let end = find_unescaped(text, &closing)?;
    let mut nodes = parse_inline(&text[..end]);
    add_mark(
        &mut nodes,
        json!({ "type": mark_type, "attrs": { "color": color } }),
    );
    Some((nodes, &text[end + closing.len()..]))
}

fn json_string(source: &str) -> Option<(String, &str)> {
    let mut values = serde_json::Deserializer::from_str(source).into_iter::<String>();
    let value = values.next()?.ok()?;
    Some((value, &source[values.byte_offset()..]))
}

fn add_mark(nodes: &mut [Value], mark: Value) {
    for node in nodes
        .iter_mut()
        .filter(|node| node.get("type") == Some(&json!("text")))
    {
        node.as_object_mut()
            .expect("text node is an object")
            .entry("marks")
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .expect("text marks are an array")
            .push(mark.clone());
    }
}

fn has_string_attr(attrs: &Map<String, Value>, key: &str) -> bool {
    attrs
        .get(key)
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
}

fn jira_block_start(line: &str) -> Option<&'static str> {
    match line {
        "{{jira:task-list}}" => Some("task-list"),
        "{{jira:decision-list}}" => Some("decision-list"),
        _ => None,
    }
}

fn jira_block_end(line: &str) -> Option<&'static str> {
    match line {
        "{{/jira:panel}}" => Some("panel"),
        "{{/jira:task-list}}" => Some("task-list"),
        "{{/jira:decision-list}}" => Some("decision-list"),
        _ => None,
    }
}

fn task_list_item(line: &str) -> Option<(&'static str, &str)> {
    line.strip_prefix("- [ ] ")
        .map(|text| ("TODO", text))
        .or_else(|| line.strip_prefix("- [x] ").map(|text| ("DONE", text)))
}

fn decision_list_item(line: &str) -> Option<&str> {
    line.strip_prefix("- ")
}

fn local_id(index: usize) -> String {
    format!("00000000-0000-4000-8000-{index:012}")
}

fn assign_list_local_ids(value: &mut Value, next_id: &mut usize) {
    let Some(node) = value.as_object_mut() else {
        return;
    };
    if matches!(
        node.get("type").and_then(Value::as_str),
        Some("taskList" | "taskItem" | "decisionList" | "decisionItem")
    ) {
        node.entry("attrs")
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .expect("list ADF attrs are an object")
            .insert("localId".into(), Value::String(local_id(*next_id)));
        *next_id += 1;
    }
    if let Some(children) = node.get_mut("content").and_then(Value::as_array_mut) {
        for child in children {
            assign_list_local_ids(child, next_id);
        }
    }
}

fn validate_inline(line: &str, line_number: usize) -> Result<(), String> {
    let mut remaining = line;
    while !remaining.is_empty() {
        if let Some(rest) = remaining.strip_prefix('\\') {
            let length = rest.chars().next().map(char::len_utf8).unwrap_or(0);
            remaining = &rest[length..];
            continue;
        }
        if remaining.starts_with("{{jira:") {
            return Err(format!(
                "legacy Jira inline tags are not supported on line {line_number}"
            ));
        }
        if remaining.starts_with("{{/jira:") {
            return Err(format!("invalid Jira block tag on line {line_number}"));
        }
        if let Some((_, rest)) = canonical_inline_node(remaining) {
            remaining = rest;
            continue;
        }
        if let Some((_, rest)) = code_inline_nodes(remaining) {
            remaining = rest;
            continue;
        }
        if let Some((_, rest)) = marked_inline_nodes(remaining, "++", "underline")
            .or_else(|| color_inline_nodes(remaining, "color", "textColor"))
        {
            remaining = rest;
            continue;
        }
        if is_canonical_inline_start(remaining) && !valid_canonical_inline(remaining) {
            return Err(format!("invalid Jira inline syntax on line {line_number}"));
        }
        let length = remaining.chars().next().map(char::len_utf8).unwrap_or(0);
        remaining = &remaining[length..];
    }
    Ok(())
}

fn valid_canonical_inline(source: &str) -> bool {
    canonical_inline_node(source).is_some()
        || marked_inline_nodes(source, "++", "underline").is_some()
        || color_inline_nodes(source, "color", "textColor").is_some()
}

fn is_canonical_inline_start(source: &str) -> bool {
    [
        "@date(",
        "@status(",
        "@mention(",
        "@card(",
        "++",
        "{color:",
        "{highlight:",
    ]
    .into_iter()
    .any(|prefix| source.starts_with(prefix))
        || source.strip_prefix(':').is_some_and(|name| {
            name.chars()
                .next()
                .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
                && name.contains(':')
        })
}

fn valid_emoji_short_name(short_name: &str) -> bool {
    !short_name.is_empty()
        && short_name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '+' | '-')
        })
}

fn valid_status_color(color: &str) -> bool {
    matches!(
        color,
        "green" | "blue" | "red" | "yellow" | "neutral" | "purple"
    )
}

fn valid_hex_color(color: &str) -> bool {
    color.len() == 7
        && color.starts_with('#')
        && color[1..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
}

fn valid_card_url(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

fn date_timestamp(date: &str) -> Option<String> {
    let (year, month, day) = parse_date(date)?;
    let days = days_since_unix_epoch(year, month, day);
    Some((days * 86_400_000).to_string())
}

fn date_from_timestamp(timestamp: &str) -> Option<String> {
    let milliseconds = timestamp.parse::<i64>().ok()?;
    (milliseconds % 86_400_000 == 0).then_some(())?;
    let (year, month, day) = civil_from_days(milliseconds / 86_400_000);
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

fn parse_date(date: &str) -> Option<(i64, u32, u32)> {
    let [year, month, day] = date.split('-').collect::<Vec<_>>().try_into().ok()?;
    (year.len() == 4 && month.len() == 2 && day.len() == 2).then_some(())?;
    let year = year.parse().ok()?;
    let month = month.parse().ok()?;
    let day = day.parse().ok()?;
    (1..=12).contains(&month).then_some(())?;
    (1..=days_in_month(year, month))
        .contains(&day)
        .then_some(())?;
    Some((year, month, day))
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    }
}

fn days_since_unix_epoch(year: i64, month: u32, day: u32) -> i64 {
    let year = year - (month <= 2) as i64;
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year =
        (153 * (month as i64 + if month > 2 { -3 } else { 9 }) + 2) / 5 + day as i64 - 1;
    era * 146_097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year - 719_468
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = month_index + if month_index < 10 { 3 } else { -9 };
    (year + (month <= 2) as i64, month as u32, day as u32)
}

fn json_attrs(source: &str) -> Option<(Map<String, Value>, &str)> {
    let mut values = serde_json::Deserializer::from_str(source).into_iter::<Value>();
    let attrs = values.next()?.ok()?.as_object()?.clone();
    Some((attrs, &source[values.byte_offset()..]))
}

fn next_inline_marker(source: &str) -> Option<usize> {
    [
        "{{",
        "**",
        "~~",
        "++",
        "`",
        "*",
        "[",
        "@date(",
        "@status(",
        "@mention(",
        "@card(",
        ":",
        "{color:",
        "{highlight:",
        "\n",
    ]
    .into_iter()
    .filter_map(|marker| find_unescaped(source, marker))
    .min()
}

fn escape_canonical_literals(text: &str) -> String {
    let mut output = String::new();
    let mut remaining = text;
    while !remaining.is_empty() {
        if [
            "@date(",
            "@status(",
            "@mention(",
            "@card(",
            "{color:",
            "{highlight:",
        ]
        .into_iter()
        .any(|prefix| remaining.starts_with(prefix))
            || remaining.starts_with("++")
            || emoji_inline_node(remaining).is_some()
        {
            output.push('\\');
        }
        let character = remaining.chars().next().expect("remaining is not empty");
        output.push(character);
        remaining = &remaining[character.len_utf8()..];
    }
    output
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
    let is_mention = node_type(node) == Some("mention");
    let is_decision_item = node_type(node) == Some("decisionItem");
    if let Some(attrs) = node.get_mut("attrs").and_then(Value::as_object_mut) {
        attrs.remove("localId");
        if is_mention {
            attrs.remove("accessLevel");
        }
        if is_decision_item {
            attrs.remove("state");
        }
        if attrs.is_empty() {
            node.remove("attrs");
        }
    }
    if let Some(marks) = node.get_mut("marks").and_then(Value::as_array_mut) {
        for mark in marks.iter_mut().filter_map(Value::as_object_mut) {
            if let Some(attrs) = mark.get_mut("attrs").and_then(Value::as_object_mut) {
                attrs.remove("localId");
                if attrs.is_empty() {
                    mark.remove("attrs");
                }
            }
        }
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

fn valid_mention(node: &Map<String, Value>) -> bool {
    let Some(attrs) = node.get("attrs").and_then(Value::as_object) else {
        return false;
    };
    has_string_attr(attrs, "id")
        && has_string_attr(attrs, "text")
        && attrs
            .keys()
            .all(|key| matches!(key.as_str(), "id" | "text" | "accessLevel" | "localId"))
}

fn valid_emoji(node: &Map<String, Value>) -> bool {
    let Some(attrs) = node.get("attrs").and_then(Value::as_object) else {
        return false;
    };
    attrs
        .get("shortName")
        .and_then(Value::as_str)
        .and_then(|short_name| {
            short_name
                .strip_prefix(':')
                .and_then(|name| name.strip_suffix(':'))
        })
        .is_some_and(valid_emoji_short_name)
        && attrs
            .keys()
            .all(|key| matches!(key.as_str(), "shortName" | "localId"))
}

fn valid_date(node: &Map<String, Value>) -> bool {
    let Some(attrs) = node.get("attrs").and_then(Value::as_object) else {
        return false;
    };
    attrs
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(date_from_timestamp)
        .is_some()
        && attrs
            .keys()
            .all(|key| matches!(key.as_str(), "timestamp" | "localId"))
}

fn valid_status(node: &Map<String, Value>) -> bool {
    let Some(attrs) = node.get("attrs").and_then(Value::as_object) else {
        return false;
    };
    has_string_attr(attrs, "text")
        && attrs
            .get("color")
            .and_then(Value::as_str)
            .is_some_and(valid_status_color)
        && attrs
            .keys()
            .all(|key| matches!(key.as_str(), "text" | "color" | "localId"))
}

fn valid_card(node: &Map<String, Value>) -> bool {
    let Some(attrs) = node.get("attrs").and_then(Value::as_object) else {
        return false;
    };
    attrs
        .get("url")
        .and_then(Value::as_str)
        .is_some_and(valid_card_url)
        && attrs
            .keys()
            .all(|key| matches!(key.as_str(), "url" | "localId"))
}

fn valid_color_mark(mark: &Map<String, Value>) -> bool {
    let Some(attrs) = mark.get("attrs").and_then(Value::as_object) else {
        return false;
    };
    attrs
        .get("color")
        .and_then(Value::as_str)
        .is_some_and(valid_hex_color)
        && attrs.keys().all(|key| key == "color")
}

fn valid_task_list(node: &Map<String, Value>) -> bool {
    attrs_are(node, &["localId"])
        && content(node).iter().all(|item| {
            item.as_object().is_some_and(|item| {
                node_type(item) == Some("taskItem")
                    && item
                        .get("attrs")
                        .and_then(|attrs| attrs.get("state"))
                        .and_then(Value::as_str)
                        .is_some_and(|state| matches!(state, "TODO" | "DONE"))
                    && attrs_are(item, &["state", "localId"])
            })
        })
}

fn valid_decision_list(node: &Map<String, Value>) -> bool {
    attrs_are(node, &["localId"])
        && content(node).iter().all(|item| {
            item.as_object().is_some_and(|item| {
                node_type(item) == Some("decisionItem")
                    && item
                        .get("attrs")
                        .and_then(|attrs| attrs.get("state"))
                        .and_then(Value::as_str)
                        .is_some_and(|state| matches!(state, "DECIDED" | "UNDECIDED"))
                    && attrs_are(item, &["state", "localId"])
            })
        })
}

fn attrs_are(node: &Map<String, Value>, allowed: &[&str]) -> bool {
    node.get("attrs")
        .and_then(Value::as_object)
        .is_none_or(|attrs| attrs.keys().all(|key| allowed.contains(&key.as_str())))
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
