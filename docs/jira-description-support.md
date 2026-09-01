# Jira Description Support

Finery edits Jira descriptions as Markdown and converts them to and from Jira ADF. It only overwrites an existing description when the source ADF can round-trip without loss. The MCP submission escape hatch requires explicit user acceptance when this safety check fails.

Before any Jira write, Finery validates every selected changed description and checks every existing ticket for a Jira conflict. A malformed Finery tag or unsafe source description blocks the entire selected change set before Jira receives a request. Jira can still fail after writes begin, so Finery reports any remote partial result rather than attempting an unsafe rollback.

## Supported Round-Trips

- Paragraphs, headings, blockquotes, code blocks, horizontal rules, and links.
- Bold, emphasis, strikethrough, and inline code.
- Flat and nested ordered or bullet lists.
- Basic tables: one header row, body rows, and one paragraph per cell with supported inline formatting.
- Panels, rendered as `{{jira:panel {"panelType":"info"}}}` blocks closed by `{{/jira:panel}}`.
- Jira mentions and smart links, rendered as Finery lossless tags such as `{{jira:mention {"id":"...","text":"@Ada"} /}}` and `{{jira:inline-card {"url":"https://example.com"} /}}`.

Escape a literal `{{` as `\{\{` so Finery does not treat it as a Jira tag.

## Guarded ADF

- Images, attachments, embeds, and other media.
- Underline, text colour, and text background colour.
- Emoji.
- Advanced tables: merged cells, dimensions, cell styling, or multi-block cell content.
- Other unsupported Jira ADF nodes, including task and decision lists, dates, status lozenges, expands, layouts, and macros.
