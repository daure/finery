use serde_json::Value;

pub(super) fn clean_for_speed_reader(source: &str) -> String {
    let mut block = None;
    let mut in_code_block = false;
    let lines = source
        .lines()
        .filter_map(|line| {
            if line.trim_start().starts_with("```") {
                in_code_block = !in_code_block;
                return Some(line.to_owned());
            }
            if in_code_block {
                return Some(line.to_owned());
            }
            if let Some(label) = panel_label(line.trim()) {
                block = Some(JiraBlock::Panel);
                return Some(label);
            }
            if line.trim() == "{{jira:task-list}}" {
                block = Some(JiraBlock::TaskList);
                return None;
            }
            if line.trim() == "{{jira:decision-list}}" {
                block = Some(JiraBlock::DecisionList);
                return None;
            }
            if matches!(
                line.trim(),
                "{{/jira:panel}}" | "{{/jira:task-list}}" | "{{/jira:decision-list}}"
            ) {
                block = None;
                return None;
            }
            let line = clean_inline(line);
            Some(match block {
                Some(JiraBlock::TaskList) if line.starts_with("- [ ] ") => {
                    format!("- To do: {}", &line[6..])
                }
                Some(JiraBlock::TaskList) if line.starts_with("- [x] ") => {
                    format!("- Done: {}", &line[6..])
                }
                Some(JiraBlock::DecisionList) if line.starts_with("- ") => {
                    format!("- Decision: {}", &line[2..])
                }
                _ => line,
            })
        })
        .collect::<Vec<_>>();
    lines.join("\n").trim().to_owned()
}

#[derive(Clone, Copy)]
enum JiraBlock {
    Panel,
    TaskList,
    DecisionList,
}

fn panel_label(line: &str) -> Option<String> {
    let attrs = line
        .strip_prefix("{{jira:panel ")?
        .strip_suffix("}}")
        .and_then(|attrs| serde_json::from_str::<Value>(attrs).ok())?;
    let panel_type = attrs.get("panelType")?.as_str()?;
    Some(format!("{}:", title_case(panel_type)))
}

fn title_case(value: &str) -> String {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return String::new();
    };
    first.to_uppercase().collect::<String>() + characters.as_str()
}

fn clean_inline(source: &str) -> String {
    let mut output = String::new();
    let mut remaining = source;
    while !remaining.is_empty() {
        if let Some(rest) = remaining.strip_prefix('\\') {
            if let Some(character) = rest.chars().next() {
                output.push(character);
                remaining = &rest[character.len_utf8()..];
                continue;
            }
        }
        if let Some((code, rest)) = inline_code(remaining) {
            output.push_str(code);
            remaining = rest;
            continue;
        }
        if let Some((replacement, rest)) = jira_inline(remaining) {
            output.push_str(&replacement);
            remaining = rest;
            continue;
        }
        let character = remaining.chars().next().expect("source is not empty");
        output.push(character);
        remaining = &remaining[character.len_utf8()..];
    }
    output
}

fn inline_code(source: &str) -> Option<(&str, &str)> {
    let code = source.strip_prefix('`')?;
    let end = code.find('`')?;
    Some((&source[..end + 2], &code[end + 1..]))
}

fn jira_inline(source: &str) -> Option<(String, &str)> {
    date(source)
        .or_else(|| status(source))
        .or_else(|| mention(source))
        .or_else(|| card(source))
        .or_else(|| color(source))
        .or_else(|| underline(source))
        .or_else(|| emoji(source))
}

fn date(source: &str) -> Option<(String, &str)> {
    let value = source.strip_prefix("@date(")?;
    let end = value.find(')')?;
    Some((value[..end].to_owned(), &value[end + 1..]))
}

fn status(source: &str) -> Option<(String, &str)> {
    let value = source.strip_prefix("@status(")?;
    let (text, rest) = json_string(value)?;
    let rest = rest.strip_prefix(", ")?;
    let end = rest.find(')')?;
    Some((text, &rest[end + 1..]))
}

fn mention(source: &str) -> Option<(String, &str)> {
    let value = source.strip_prefix("@mention(")?;
    let (text, rest) = json_string(value)?;
    let rest = rest.strip_prefix(", ")?;
    let (_, rest) = json_string(rest)?;
    Some((text, rest.strip_prefix(')')?))
}

fn card(source: &str) -> Option<(String, &str)> {
    let value = source.strip_prefix("@card(")?;
    let end = value.find(')')?;
    Some((value[..end].to_owned(), &value[end + 1..]))
}

fn color(source: &str) -> Option<(String, &str)> {
    let value = source.strip_prefix("{color:")?;
    let color_end = value.find('}')?;
    let value = &value[color_end + 1..];
    let end = value.find("{/color}")?;
    Some((
        clean_inline(&value[..end]),
        &value[end + "{/color}".len()..],
    ))
}

fn underline(source: &str) -> Option<(String, &str)> {
    let value = source.strip_prefix("++")?;
    let end = value.find("++")?;
    Some((clean_inline(&value[..end]), &value[end + 2..]))
}

fn emoji(source: &str) -> Option<(String, &str)> {
    let value = source.strip_prefix(':')?;
    let end = value.find(':')?;
    let name = &value[..end];
    (!name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || character == '_'
                || character == '-'
                || character == '+'
        }))
    .then(|| (name.replace(['_', '-'], " "), &value[end + 1..]))
}

fn json_string(source: &str) -> Option<(String, &str)> {
    let mut values = serde_json::Deserializer::from_str(source).into_iter::<String>();
    let value = values.next()?.ok()?;
    Some((value, &source[values.byte_offset()..]))
}
