# Jira Description Support

Finery edits Jira descriptions as Markdown and converts them to and from Jira ADF. It only overwrites an existing description when the source ADF can round-trip without loss. The MCP submission escape hatch requires explicit user acceptance when this safety check fails.

Finery rejects a Composer patch atomically when any changed description contains malformed Jira syntax. Before any Jira write, it validates every selected changed description and checks every existing ticket for a Jira conflict. A malformed Finery tag or unsafe source description blocks the entire selected change set before Jira receives a request. Jira can still fail after writes begin, so Finery reports any remote partial result rather than attempting an unsafe rollback.

## Supported Round-Trips

- Paragraphs, headings, blockquotes, code blocks, horizontal rules, and links.
- Bold, emphasis, strikethrough, inline code, underline (`++text++`), and text colour (`{color:#RRGGBB}text{/color}`). Jira background highlights render through the REST API but break Jira's native editor, so Finery rejects them.
- Flat and nested ordered or bullet lists.
- Basic tables: one header row, body rows, and one paragraph per cell with supported inline formatting.
- Panels, rendered as `{{jira:panel {"panelType":"info"}}}` blocks closed by `{{/jira:panel}}`.
- Jira emoji as `:short_name:`.
- Jira dates at UTC midnight as `@date(YYYY-MM-DD)`.
- Jira status lozenges as `@status("Text", color)`, where color is `green`, `blue`, `red`, `yellow`, `neutral`, or `purple`.
- Jira mentions as `@mention("@Name", "ACCOUNT_ID")` and smart links as `@card(https://example.com)`.
- Jira task lists as `{{jira:task-list}}` blocks with `- [ ]` or `- [x]` items, and decision lists as `{{jira:decision-list}}` blocks with plain `- item` entries. Both require their matching closing tag.

Escape a literal `{{` as `\{\{`. Escape any literal canonical inline opening (`@date(`, `@status(`, `@mention(`, `@card(`, `:short_name:`, `++`, `{color:`, or `{highlight:`) with a leading backslash. Emoji syntax needs both colons, and content in inline code spans is literal. ADF rendering adds these escapes for literal source text. The old `{{jira:mention ... /}}` and `{{jira:inline-card ... /}}` forms are rejected.

## Guarded ADF

- Images, attachments, embeds, and other media.
- Advanced tables: merged cells, dimensions, cell styling, or multi-block cell content.
- Dates with a time of day or invalid timestamp.
- Text background highlights (`backgroundColor`).
- Other unsupported Jira ADF nodes, including expands, layouts, and macros.
