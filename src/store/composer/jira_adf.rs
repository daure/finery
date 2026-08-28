use serde_json::{Map, Value, json};

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

pub(crate) fn adf_is_safe_to_overwrite(value: &Value) -> bool {
    matches!(value, Value::Null | Value::String(_))
        || (value.is_object()
            && normalize_adf(markdown_to_adf(&adf_to_markdown(value)))
                == normalize_adf(value.clone()))
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
        "panel" => render_blocks(content(node)),
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

fn render_list(items: &[Value], ordered_start: Option<usize>) -> String {
    items
        .iter()
        .filter_map(Value::as_object)
        .enumerate()
        .map(|(index, item)| {
            let marker = ordered_start
                .map(|start| format!("{}. ", start + index))
                .unwrap_or_else(|| "- ".into());
            let body = render_blocks(content(item));
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

fn render_inline(nodes: &[Value]) -> String {
    nodes
        .iter()
        .filter_map(Value::as_object)
        .map(|node| match node_type(node).unwrap_or_default() {
            "text" => render_text(node),
            "hardBreak" => "  \n".into(),
            "mention" => node
                .get("attrs")
                .and_then(|attrs| attrs.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("@mention")
                .to_owned(),
            "emoji" => node
                .get("attrs")
                .and_then(|attrs| attrs.get("text").or_else(|| attrs.get("shortName")))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            "inlineCard" => node
                .get("attrs")
                .and_then(|attrs| attrs.get("url"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            _ => render_inline(content(node)),
        })
        .collect()
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

fn escape_markdown(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('*', "\\*")
        .replace('_', "\\_")
        .replace('`', "\\`")
        .replace('[', "\\[")
        .replace(']', "\\]")
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
        if unordered_item(line).is_some() {
            let (list, next) = parse_list(&lines, index, false);
            blocks.push(list);
            index = next;
            continue;
        }
        if ordered_item(line).is_some() {
            let (list, next) = parse_list(&lines, index, true);
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
        || unordered_item(line).is_some()
        || ordered_item(line).is_some()
        || line.trim_start().starts_with("> ")
        || line.trim().starts_with("```")
        || line.trim() == "---"
}

fn unordered_item(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    ["- ", "* ", "+ "]
        .into_iter()
        .find_map(|prefix| trimmed.strip_prefix(prefix))
}

fn ordered_item(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    let number = trimmed.get(..digits)?.parse().ok()?;
    let text = trimmed.get(digits..)?.strip_prefix(". ")?;
    Some((number, text))
}

fn parse_list(lines: &[&str], start: usize, ordered: bool) -> (Value, usize) {
    let mut index = start;
    let mut items = Vec::new();
    let mut order = 1;
    while index < lines.len() {
        let item = if ordered {
            let Some((number, text)) = ordered_item(lines[index]) else {
                break;
            };
            if items.is_empty() {
                order = number;
            }
            text
        } else {
            let Some(text) = unordered_item(lines[index]) else {
                break;
            };
            text
        };
        items.push(json!({
            "type": "listItem",
            "content": [{
                "type": "paragraph",
                "content": parse_inline(item),
            }],
        }));
        index += 1;
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

fn next_inline_marker(source: &str) -> Option<usize> {
    ["**", "~~", "`", "*", "[", "\n"]
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
