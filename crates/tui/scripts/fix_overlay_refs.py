#!/usr/bin/env python3
"""Bulk-update TUI sources after UiOverlay FSM migration (read paths)."""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path("/Volumes/app/seriouse-project/native-cli-ai/crates/tui")

FILES = [
    ROOT / "src/tui/app.rs",
    ROOT / "src/repl/tui_mode.rs",
    ROOT / "src/tui/transcript.rs",
    ROOT / "src/tui/onboarding.rs",
]

CLOSE = {
    "command_palette_open": "close_command_palette",
    "branch_picker_open": "close_branch_picker",
    "connect_modal_open": "close_connect_modal",
    "api_key_modal_open": "close_api_key_modal",
    "info_modal_open": "close_info_modal",
    "model_picker_open": "close_model_picker",
    "permission_picker_open": "close_permission_picker",
    "agent_picker_open": "close_agent_picker",
    "question_modal_open": "close_question_modal",
    "session_picker_open": "close_session_picker",
    "provider_picker_open": "close_provider_picker",
    "custom_provider_setup_open": "close_custom_provider_setup",
}

OPEN = {
    "command_palette_open": "open_command_palette",
    "branch_picker_open": "open_branch_picker",
    "connect_modal_open": "open_connect_modal",
    "api_key_modal_open": "open_api_key_modal",
    "info_modal_open": "open_info_modal",
    "model_picker_open": "open_model_picker",
    "permission_picker_open": "open_permission_picker",
    "agent_picker_open": "open_agent_picker",
    "question_modal_open": "open_question_modal",
    "session_picker_open": "open_session_picker",
    "provider_picker_open": "open_provider_picker",
    "custom_provider_setup_open": "open_custom_provider_setup",
}

GETTERS = list(CLOSE.keys()) + [
    "command_palette_query",
    "palette_index",
    "branch_picker_query",
    "branch_picker_index",
    "branch_picker_branches",
    "connect_search",
    "connect_menu_index",
    "connect_modal_scroll",
    "api_key_target_provider",
    "api_key_input",
    "api_key_target_has_existing",
    "api_key_connect_after_save",
    "info_modal_title",
    "info_modal_lines",
    "info_modal_scroll",
    "model_picker_search",
    "model_picker_index",
    "model_picker_entries",
    "model_picker_scroll",
    "permission_picker_index",
    "agent_picker_index",
    "question_modal_index",
    "question_modal_scroll",
    "session_picker_search",
    "session_picker_index",
    "session_picker_entries",
    "session_picker_scroll",
    "provider_picker_index",
    "provider_picker_scroll",
    "provider_picker_for_api_key",
    "provider_picker_include_add_row",
    "custom_provider_setup_step",
    "custom_setup_compat_index",
    "custom_setup_input",
    "custom_setup_base_url",
    "custom_setup_api_key",
    "custom_setup_model_hint",
]


def transform(text: str) -> str:
    for name, close in CLOSE.items():
        text = re.sub(rf"(\w+)\.{name}\s*=\s*false\s*;", rf"\1.{close}();", text)
    for name, open_fn in OPEN.items():
        text = re.sub(rf"(\w+)\.{name}\s*=\s*true\s*;", rf"\1.{open_fn}();", text)

    for name in GETTERS:
        text = re.sub(rf"(\w+)\.{name}(?!\()", rf"\1.{name}()", text)

    return text


def main() -> None:
    for path in FILES:
        if not path.exists():
            continue
        original = path.read_text()
        updated = transform(original)
        if updated != original:
            path.write_text(updated)
            print(f"updated {path}")


if __name__ == "__main__":
    main()
