#!/usr/bin/env python3
"""Fix mutable overlay field accesses after getter migration."""

from __future__ import annotations

import re
from pathlib import Path

FILES = [
    Path("/Volumes/app/seriouse-project/native-cli-ai/crates/tui/src/tui/app.rs"),
    Path("/Volumes/app/seriouse-project/native-cli-ai/crates/tui/src/repl/tui_mode.rs"),
]

MUT_STRING = [
    "command_palette_query",
    "branch_picker_query",
    "connect_search",
    "api_key_input",
    "model_picker_search",
    "session_picker_search",
    "custom_setup_input",
    "custom_setup_base_url",
    "custom_setup_api_key",
    "custom_setup_model_hint",
]

MUT_USIZE = [
    "palette_index",
    "branch_picker_index",
    "connect_menu_index",
    "connect_modal_scroll",
    "info_modal_scroll",
    "model_picker_index",
    "model_picker_scroll",
    "permission_picker_index",
    "agent_picker_index",
    "question_modal_index",
    "question_modal_scroll",
    "session_picker_index",
    "session_picker_scroll",
    "provider_picker_index",
    "provider_picker_scroll",
    "custom_setup_compat_index",
]


def transform(text: str) -> str:
    for name in MUT_STRING:
        text = re.sub(
            rf"(\w+)\.{name}\(\)\.(push|pop|clear)\(",
            rf"\1.{name}_mut().unwrap().\2(",
            text,
        )
        text = re.sub(
            rf"(\w+)\.{name}\(\)\.(push|pop|clear)\(\)",
            rf"\1.{name}_mut().unwrap().\2()",
            text,
        )

    for name in MUT_USIZE:
        text = re.sub(
            rf"(\w+)\.{name}\(\)\s*=",
            rf"*\\1.{name}_mut().unwrap() =",
            text,
        )

    # model_picker_entries = entries -> handled by open_model_picker
    text = text.replace(
        "g.model_picker_entries() = entries;",
        "// entries set via open_model_picker",
    )

    return text


def main() -> None:
    for path in FILES:
        original = path.read_text()
        updated = transform(original)
        if updated != original:
            path.write_text(updated)
            print(path)


if __name__ == "__main__":
    main()
