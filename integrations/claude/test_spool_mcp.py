"""Tests for spool_mcp against a fake engine socket. No audio, no OpenAI calls.

Run from this folder:
  uv run --with "mcp>=2.3,<3" --with pytest pytest -q
"""

# The fake engine always sends the optional reply keys these tests read.
# pyright: reportTypedDictNotRequiredAccess=false

from __future__ import annotations

import asyncio
import json
import shutil
import socket
import tempfile
import threading
from collections.abc import Iterator
from dataclasses import dataclass, field
from pathlib import Path

import pytest
from mcp.server.mcpserver.exceptions import ToolError

import spool_mcp

STATUS = {"speaking": False, "paused": False, "queued": 0, "loading": 0, "stalled": False, "device": "Fake"}


@dataclass
class FakeEngine:
    """Answers each JSON line with the next canned reply, recording what it got."""

    path: Path
    # A dict is sent as JSON, a str as a raw line, None as a hang-up with no reply.
    replies: list[dict[str, object] | str | None] = field(default_factory=list)
    received: list[dict[str, object]] = field(default_factory=list)

    def serve(self, server: socket.socket) -> None:
        while True:
            try:
                connection, _ = server.accept()
            except OSError:
                return
            with connection:
                line = connection.makefile("r").readline()
                self.received.append(json.loads(line))
                reply = self.replies.pop(0) if self.replies else {"ok": True, **STATUS}
                if isinstance(reply, str):
                    connection.sendall(reply.encode() + b"\n")
                elif reply is not None:
                    connection.sendall(json.dumps(reply).encode() + b"\n")


@pytest.fixture
def home(monkeypatch: pytest.MonkeyPatch) -> Iterator[Path]:
    # A short folder of our own: unix socket paths are capped at 104 bytes on macOS.
    folder = Path(tempfile.mkdtemp(prefix="spool"))
    monkeypatch.setenv("SPOOL_SOCKET", str(folder / "s.sock"))
    monkeypatch.setenv("SPOOL_LIBRARY_DIR", str(folder / "library"))
    monkeypatch.setenv("SPOOL_APP", str(folder / "NoSuch.app"))
    yield folder
    shutil.rmtree(folder)


@pytest.fixture
def engine(home: Path) -> Iterator[FakeEngine]:
    fake = FakeEngine(home / "s.sock")
    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(str(fake.path))
    server.listen()
    thread = threading.Thread(target=fake.serve, args=(server,), daemon=True)
    thread.start()
    yield fake
    server.close()


@pytest.fixture
def no_start(monkeypatch: pytest.MonkeyPatch) -> list[str]:
    """Stands in for opening the app, so a test never launches the real Spool."""
    starts: list[str] = []
    monkeypatch.setattr(spool_mcp, "start_engine", lambda: starts.append("start"))
    monkeypatch.setattr(spool_mcp, "STARTUP_TIMEOUT_S", 0.5)
    return starts


def saved_reading(home: Path, name: str, chunk_texts: list[str]) -> Path:
    folder = home / "library" / name
    folder.mkdir(parents=True)
    (folder / "transcript.md").write_text(f"# {name} title\n\n{' '.join(chunk_texts)}\n")
    for i, text in enumerate(chunk_texts, start=1):
        (folder / f"{i:03}.mp3").write_bytes(b"")
        (folder / f"{i:03}.txt").write_text(text)
    return folder


def test_all_six_tools_are_registered() -> None:
    tools = asyncio.run(spool_mcp.mcp.list_tools())
    names = sorted(tool.name for tool in tools)
    assert names == [
        "spool_arrange", "spool_control", "spool_library", "spool_queue", "spool_replay", "spool_speak",
    ]


def test_read_only_tools_are_marked_read_only() -> None:
    tools = {tool.name: tool for tool in asyncio.run(spool_mcp.mcp.list_tools())}
    read_only = {name for name, tool in tools.items() if tool.annotations and tool.annotations.read_only_hint}
    assert read_only == {"spool_library", "spool_queue"}


def test_speak_sends_text_and_saves_transcript(engine: FakeEngine, home: Path) -> None:
    engine.replies = [{"ok": True, "chunks": 1, **STATUS}]

    reply = spool_mcp.spool_speak("Hello there, world.", title="Greeting", voice="ash", position="next")

    sent = engine.received[0]
    folder = home / "library" / reply["saved_as"]
    assert sent == {
        "cmd": "speak", "text": "Hello there, world.", "save_dir": str(folder),
        "title": "Greeting", "at": "next", "voice": "ash",
    }
    assert reply["ok"] and reply["chunks"] == 1
    assert reply["saved_as"].endswith("-greeting")
    assert (folder / "transcript.md").read_text() == "# Greeting\n\nHello there, world.\n"


def test_speak_without_title_uses_opening_words_and_leaves_out_voice(engine: FakeEngine) -> None:
    spool_mcp.spool_speak("One two three four five six seven eight nine ten")

    sent = engine.received[0]
    assert sent["title"] == "One two three four five six seven eight"
    assert "voice" not in sent and "instructions" not in sent
    assert sent["at"] == "last"


def test_speak_twice_in_one_minute_gets_two_folders(engine: FakeEngine) -> None:
    first = spool_mcp.spool_speak("Same words", title="Same")
    second = spool_mcp.spool_speak("Same words", title="Same")
    assert first["saved_as"] != second["saved_as"]
    assert second["saved_as"].endswith("-same-2")


def test_speak_empty_text_sends_nothing(engine: FakeEngine) -> None:
    reply = spool_mcp.spool_speak("   ")
    assert reply == {"ok": False, "error": "nothing to speak"}
    assert engine.received == []


def test_engine_error_reply_is_passed_through(engine: FakeEngine) -> None:
    engine.replies = [{"ok": False, "error": "unknown voice \"robot\"", **STATUS}]
    reply = spool_mcp.spool_speak("Hi", voice="robot")
    assert reply["ok"] is False and reply["error"] == "unknown voice \"robot\""


@pytest.mark.parametrize(
    ("action", "value", "expected"),
    [
        ("pause", 0.0, {"cmd": "pause"}),
        ("skip", 0.0, {"cmd": "skip"}),
        ("seek", -10.0, {"cmd": "seek", "by": -10.0}),
        ("speed", 1.25, {"cmd": "set_speed", "speed": 1.25}),
        ("volume", 0.5, {"cmd": "set_volume", "volume": 0.5}),
        ("clear_played", 0.0, {"cmd": "clear_played"}),
    ],
)
def test_control_commands(engine: FakeEngine, action: spool_mcp.Action, value: float, expected: dict[str, object]) -> None:
    spool_mcp.spool_control(action, value)
    assert engine.received == [expected]


def test_queue_splits_played_current_and_up_next(engine: FakeEngine) -> None:
    long_text = "word " * 40
    entries = [
        {"id": n, "reading": 1, "title": "Doc", "index": n, "of": 4, "text": f"chunk {n} " + long_text, "state": "ready"}
        for n in range(1, 5)
    ]
    engine.replies = [{"ok": True, "queue": {"entries": entries, "cursor": 2}, "position": 3.5, "speed": 1.1, **STATUS}]

    view = spool_mcp.spool_queue()

    assert engine.received == [{"cmd": "queue"}]
    assert [chunk["id"] for chunk in view["played"]] == [2, 1]
    assert view["current"] is not None and view["current"]["id"] == 3
    assert [chunk["id"] for chunk in view["up_next"]] == [4]
    assert view["current"]["chunk"] == "3/4"
    assert view["current"]["preview"].endswith("...") and len(view["current"]["preview"]) <= 93
    assert view["position"] == 3.5 and view["speed"] == 1.1


def test_queue_when_everything_has_played(engine: FakeEngine) -> None:
    entries = [{"id": 1, "reading": 1, "title": "Doc", "index": 1, "of": 1, "text": "Done.", "state": "ready"}]
    engine.replies = [{"ok": True, "queue": {"entries": entries, "cursor": 1}, **STATUS}]

    view = spool_mcp.spool_queue()

    assert view["current"] is None and view["up_next"] == []
    assert [chunk["id"] for chunk in view["played"]] == [1]


@pytest.mark.parametrize(
    ("action", "before", "expected"),
    [
        ("move", 17, {"cmd": "move", "id": 42, "before": 17}),
        ("move", None, {"cmd": "move", "id": 42}),
        ("jump", 17, {"cmd": "jump", "id": 42}),
        ("remove", None, {"cmd": "remove", "id": 42}),
    ],
)
def test_arrange_commands(engine: FakeEngine, action: spool_mcp.Arrangement, before: int | None, expected: dict[str, object]) -> None:
    spool_mcp.spool_arrange(action, 42, before)
    assert engine.received == [expected]


def test_replay_from_text_starts_at_the_chunk_holding_it(engine: FakeEngine, home: Path) -> None:
    folder = saved_reading(home, "2026-01-01-1200-notes", ["Intro here.", "Section two starts.", "The end."])

    reply = spool_mcp.spool_replay("notes", from_text="section TWO", position="now")

    assert engine.received == [{
        "cmd": "play_files",
        "paths": [str(folder / "002.mp3"), str(folder / "003.mp3")],
        "title": "2026-01-01-1200-notes title",
        "at": "now",
    }]
    assert reply["saved_as"] == folder.name


def test_replay_from_chunk_and_newest_match(engine: FakeEngine, home: Path) -> None:
    saved_reading(home, "2026-01-01-1200-notes", ["a", "b"])
    newer = saved_reading(home, "2026-02-01-1200-notes", ["c", "d", "e"])

    spool_mcp.spool_replay("notes", from_chunk=3)

    assert engine.received[0]["paths"] == [str(newer / "003.mp3")]


def test_replay_errors_send_nothing(engine: FakeEngine, home: Path) -> None:
    saved_reading(home, "2026-01-01-1200-notes", ["a"])
    (home / "library" / "2026-01-01-1300-empty").mkdir()
    (home / "library" / "2026-01-01-1300-empty" / "transcript.md").write_text("# Empty\n")

    assert spool_mcp.spool_replay("notes", from_text="zebra")["ok"] is False
    assert spool_mcp.spool_replay("2026-01-01-1300-empty")["error"] == "2026-01-01-1300-empty has no saved audio"
    with pytest.raises(ToolError, match="no saved reading matches"):
        spool_mcp.spool_replay("nothing-like-it")
    assert engine.received == []


def test_library_lists_newest_first_and_filters(home: Path) -> None:
    saved_reading(home, "2026-01-01-1200-alpha", ["first text"])
    saved_reading(home, "2026-02-01-1200-beta", ["second text", "more"])

    readings = spool_mcp.spool_library()
    assert [reading["name"] for reading in readings] == ["2026-02-01-1200-beta", "2026-01-01-1200-alpha"]
    assert readings[0]["chunks"] == 2
    assert [reading["name"] for reading in spool_mcp.spool_library("FIRST")] == ["2026-01-01-1200-alpha"]


def test_library_when_nothing_saved_yet(home: Path) -> None:
    assert spool_mcp.spool_library() == []


def test_status_when_engine_is_not_running_does_not_start_it(home: Path, no_start: list[str]) -> None:
    reply = spool_mcp.spool_control("status")
    assert reply == {"ok": False, "error": "Spool is not running"}
    assert spool_mcp.spool_queue() == {"ok": False, "error": "Spool is not running"}
    assert no_start == []


def test_speak_when_engine_never_comes_up(home: Path, no_start: list[str]) -> None:
    with pytest.raises(ToolError, match="did not start"):
        spool_mcp.spool_speak("Hello")
    assert no_start == ["start"]


def test_stale_socket_file_counts_as_not_running(home: Path, no_start: list[str]) -> None:
    (home / "s.sock").write_text("")
    assert spool_mcp.spool_control("status")["error"] == "Spool is not running"


def test_no_app_and_no_binary_says_how_to_install(home: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(spool_mcp.shutil, "which", lambda _: None)
    monkeypatch.setattr(spool_mcp.Path, "home", lambda: home)
    with pytest.raises(ToolError, match="Spool is not installed"):
        spool_mcp.start_engine()


def test_engine_hanging_up_without_a_reply(engine: FakeEngine) -> None:
    engine.replies = [None]
    assert spool_mcp.spool_control("pause") == {"ok": False, "error": "no reply from Spool"}


def test_errors_reach_claude_with_their_reason(home: Path) -> None:
    # A plain exception reaches the client as "Error executing tool X" with the reason dropped.
    with pytest.raises(ToolError, match="no saved reading matches 'nothing-like-it'"):
        asyncio.run(spool_mcp.mcp.call_tool("spool_replay", {"name": "nothing-like-it"}))


def test_queue_shows_only_the_most_recent_played(engine: FakeEngine) -> None:
    entries = [
        {"id": n, "reading": 1, "title": "Doc", "index": n, "of": 30, "text": "Words.", "state": "ready"}
        for n in range(1, 31)
    ]
    engine.replies = [{"ok": True, "queue": {"entries": entries, "cursor": 25}, **STATUS}]

    view = spool_mcp.spool_queue()

    assert [chunk["id"] for chunk in view["played"]] == list(range(25, 15, -1))
    assert view["played_total"] == 25
    assert [chunk["id"] for chunk in view["up_next"]] == [27, 28, 29, 30]


def test_failed_speak_leaves_no_empty_reading(engine: FakeEngine, home: Path) -> None:
    engine.replies = [{"ok": False, "error": "no API key", **STATUS}]
    spool_mcp.spool_speak("Hello", title="Broken")
    assert spool_mcp.spool_library() == []


def test_speak_that_never_reaches_spool_leaves_no_empty_reading(home: Path, no_start: list[str]) -> None:
    with pytest.raises(ToolError):
        spool_mcp.spool_speak("Hello")
    assert spool_mcp.spool_library() == []


def test_garbled_reply_is_a_tool_error_with_a_reason(engine: FakeEngine) -> None:
    engine.replies = ['{"ok": tru']
    with pytest.raises(ToolError, match="unreadable reply from Spool"):
        spool_mcp.spool_control("pause")


def test_replay_past_the_last_chunk_is_an_error(engine: FakeEngine, home: Path) -> None:
    saved_reading(home, "2026-01-01-1200-notes", ["a", "b"])
    reply = spool_mcp.spool_replay("notes", from_chunk=5)
    assert reply == {"ok": False, "error": "2026-01-01-1200-notes has 2 chunks, not 5"}
    assert engine.received == []
