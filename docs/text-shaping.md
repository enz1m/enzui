# Text shaping policy

`EnzText` and `MeasureTypography` use the shared `TextShaping` policy. Their
compatibility default remains `Auto`: ASCII selects Basic while any non-ASCII
content selects Advanced. Callers rendering prose can request `Advanced` to
match rich text and prevent kerning, ligatures, widths, and wrap points from
changing merely because translated punctuation introduces one non-ASCII
character.

Use `TextShaping::Basic` only for text whose font and character repertoire are
controlled and whose per-codepoint, no-ligature advances are intentional. This
is suitable for some ASCII code and fixed chrome. It does not provide font
fallback or complex-script shaping.

`EnzLabel` forwards one explicit policy to its title, subtitle, and text-glyph
icon. `EnzTextPair` forwards one policy to both string runs; its element form
forwards the policy only to the leading string because the trailing element owns
its typography. Iced's span-based rich text always uses Advanced shaping, named
by `RICH_TEXT_SHAPING`.

## Measurement and reuse

Use the same shaping policy for `MeasureTypography` and the text it measures.
Changing the policy can change glyph selection, advances and wrapping even when
the string and font size stay the same. The paragraph cache includes the shaping
policy in its comparison; unchanged paragraphs can reuse their shaped content.

Advanced shaping costs depend on the text, font fallback, renderer, and cache
state. Measure a representative application in its release profile before
changing a broad typography policy. A fresh-paragraph microbenchmark does not
represent the cost of a cached application frame.
