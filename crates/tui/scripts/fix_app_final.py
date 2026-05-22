#!/usr/bin/env python3
"""Final app.rs overlay migration fixes."""

from pathlib import Path
import re

path = Path("/Volumes/app/seriouse-project/native-cli-ai/crates/tui/src/tui/app.rs")
text = path.read_text()

# unstable str_as_str
text = text.replace(".command_palette_query().as_str()", ".command_palette_query()")
text = text.replace(".connect_search().as_str()", ".connect_search()")

# missed getter calls
for field in [
    "branch_picker_index",
    "api_key_target_provider",
    "model_picker_entries",
    "model_picker_index",
    "session_picker_entries",
]:
    text = re.sub(rf"(\w+)\.{field}(?!\()", rf"\1.{field}()", text)

# invalid -= on getter
text = text.replace("g.palette_index() -= 1;", "*g.palette_index_mut().unwrap() -= 1;")

# custom setup assignments
repls = [
    ("g.custom_provider_setup_step() =", "*g.custom_provider_setup_step_mut().unwrap() ="),
    ("g.custom_setup_base_url() =", "*g.custom_setup_base_url_mut().unwrap() ="),
    ("g.custom_setup_api_key() =", "*g.custom_setup_api_key_mut().unwrap() ="),
    ("g.custom_setup_input() =", "*g.custom_setup_input_mut().unwrap() ="),
    ("g.custom_setup_compat_index() += 1", "*g.custom_setup_compat_index_mut().unwrap() += 1"),
]
for a, b in repls:
    text = text.replace(a, b)

# draw path cache
text = text.replace(
    "let (lines, _hits) = transcript_lines_and_hits(&g, inner_w);",
    "let cache = ensure_transcript_cache(&mut g, inner_w);\n                let lines = &cache.lines;\n                let _hits = &cache.hits;",
)

path.write_text(text)
print("fixed app.rs")
