# /// script
# requires-python = ">=3.11"
# dependencies = ["mcp>=2.3,<3"]
# ///
"""MCP server that lets Claude read text aloud through Spool.

Spool's engine listens on a unix socket and takes one JSON command per line.
This server is a client of that socket, like the menu bar app and the
rust-tts command line, so whatever Claude queues shows up in the same queue
and is controlled from the same places.

Run it with `uv run --script spool_mcp.py`. Settings, all optional:

  SPOOL_SOCKET       the engine's socket
                     (default ~/Library/Application Support/rust_tts/rust_tts.sock)
  SPOOL_LIBRARY_DIR  where readings are saved
                     (default ~/Library/Application Support/rust_tts/library,
                     the folder the app saves its own readings in)
  SPOOL_APP          the app to open when the engine is not running
                     (default /Applications/Spool.app)
"""

from __future__ import annotations

import json
import os
import re
import shutil
import socket
import subprocess
import time
from datetime import datetime
from pathlib import Path
from typing import Literal, TypedDict, cast

from mcp.server.mcpserver import MCPServer
from mcp.server.mcpserver.exceptions import ToolError
from mcp.types import ToolAnnotations

SPOOL_DIR = Path.home() / "Library" / "Application Support" / "rust_tts"
TRANSCRIPT = "transcript.md"
STARTUP_TIMEOUT_S = 15.0
REPLY_TIMEOUT_S = 30.0
PREVIEW_CHARS = 90
# The played list grows all session; only the latest are worth Claude's context.
PLAYED_SHOWN = 10

Action = Literal[
    "play", "pause", "toggle", "skip", "previous", "restart", "stop", "clear_played",
    "seek", "speed", "volume", "status", "quit",
]
Arrangement = Literal["move", "jump", "remove"]
# now plays at once (the interrupted chunk waits at the top of up next), next
# after the current chunk, last after everything queued.
Position = Literal["now", "next", "last"]


class Reply(TypedDict, total=False):
    """What the engine sends back. Playback state rides along on every reply."""

    ok: bool
    error: str
    chunks: int
    speaking: bool
    paused: bool
    queued: int
    loading: int
    stalled: bool
    device: str
    position: float
    speed: float
    volume: float
    saved_as: str


class Chunk(TypedDict):
    """One row of the app's lists."""

    id: int
    title: str
    chunk: str  # "3/7": its place in its reading
    state: str  # ready, loading or failed
    preview: str


class QueueView(TypedDict, total=False):
    ok: bool
    error: str
    current: Chunk | None
    played: list[Chunk]  # most recent first, at most PLAYED_SHOWN
    played_total: int
    up_next: list[Chunk]  # in play order
    paused: bool
    position: float
    speed: float


class Reading(TypedDict):
    name: str
    title: str
    chunks: int
    chars: int


def socket_path() -> Path:
    return Path(os.environ.get("SPOOL_SOCKET") or SPOOL_DIR / "rust_tts.sock").expanduser()


def library_dir() -> Path:
    return Path(os.environ.get("SPOOL_LIBRARY_DIR") or SPOOL_DIR / "library").expanduser()


def connect() -> socket.socket | None:
    """A live connection, or None when nothing is listening."""
    path = socket_path()
    if not path.exists():
        return None

    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.settimeout(REPLY_TIMEOUT_S)
    try:
        client.connect(str(path))
    except OSError:
        client.close()
        return None
    return client


def start_engine() -> None:
    """Open Spool in the background, which starts its engine.

    Through the app rather than the bare engine, so macOS counts Spool as
    responsible for it and its Accessibility grant covers the hotkey. The
    rust-tts binary is the fallback when the app is not installed; it reads
    OPENAI_API_KEY from this process's environment.
    """
    app = Path(os.environ.get("SPOOL_APP") or "/Applications/Spool.app").expanduser()
    if app.exists():
        opened = subprocess.run(["open", "-g", "-j", str(app)], capture_output=True, text=True)
        if opened.returncode != 0:
            raise ToolError(f"could not open {app}: {opened.stderr.strip()}")
        return

    binary = shutil.which("rust-tts") or str(Path.home() / ".local" / "bin" / "rust-tts")
    if not Path(binary).is_file():
        raise ToolError(f"Spool is not installed: no {app} and no rust-tts binary. Run build.sh.")

    log_path = SPOOL_DIR / "rust_tts.log"
    log_path.parent.mkdir(parents=True, exist_ok=True)
    with log_path.open("a", encoding="utf-8") as log:
        subprocess.Popen(
            [binary], stdin=subprocess.DEVNULL, stdout=log, stderr=log, start_new_session=True
        )


def ensure_running() -> socket.socket:
    """A connection to the engine, starting Spool first if it is not up."""
    client = connect()
    if client is not None:
        return client

    start_engine()
    deadline = time.monotonic() + STARTUP_TIMEOUT_S
    while time.monotonic() < deadline:
        time.sleep(0.2)
        client = connect()
        if client is not None:
            return client

    raise ToolError(f"Spool did not start within {STARTUP_TIMEOUT_S:.0f}s")


def send(command: dict[str, object], *, start_if_down: bool = True) -> Reply:
    """Send one command and return the engine's reply."""
    client = ensure_running() if start_if_down else connect()
    if client is None:
        return Reply(ok=False, error="Spool is not running")

    received = b""
    try:
        client.sendall(json.dumps(command).encode("utf-8") + b"\n")
        while b"\n" not in received:
            block = client.recv(4096)
            if not block:
                break
            received += block
    except OSError as error:
        raise ToolError(f"lost the connection to Spool: {error}") from error
    finally:
        client.close()

    raw = received.decode("utf-8").strip()
    if not raw:
        return Reply(ok=False, error="no reply from Spool")
    try:
        return cast(Reply, json.loads(raw))
    except json.JSONDecodeError as error:
        raise ToolError(f"unreadable reply from Spool: {raw[:200]}") from error


def slug(text: str, words: int = 8) -> str:
    """Lowercase words joined by hyphens, for a folder name a person can read."""
    found: list[str] = re.findall(r"[a-z0-9]+", text.lower())
    return "-".join(found[:words]) or "reading"


def title_of(folder: Path) -> str:
    transcript = folder / TRANSCRIPT
    if not transcript.exists():
        return ""
    first_line = transcript.read_text(encoding="utf-8").partition("\n")[0]
    return first_line.removeprefix("# ").strip()


def new_reading_folder(text: str, title: str) -> Path:
    """A fresh folder in the library, named by time and opening words, holding the transcript."""
    stamp = datetime.now().strftime("%Y-%m-%d-%H%M")
    base = f"{stamp}-{slug(title or text)}"
    folder = library_dir() / base
    n = 2
    while folder.exists():
        folder = library_dir() / f"{base}-{n}"
        n += 1
    folder.mkdir(parents=True)

    heading = title.strip() or " ".join(text.split()[:8])
    (folder / TRANSCRIPT).write_text(f"# {heading}\n\n{text.strip()}\n", encoding="utf-8")
    return folder


def find_reading(name: str) -> Path:
    """The folder `name` points at: an exact name, or the newest one containing it."""
    exact = library_dir() / name
    if exact.is_dir():
        return exact

    matches = [reading["name"] for reading in list_readings() if name.lower() in reading["name"].lower()]
    if not matches:
        raise ToolError(f"no saved reading matches {name!r}")
    return library_dir() / matches[0]


def list_readings(query: str = "") -> list[Reading]:
    folder_root = library_dir()
    if not folder_root.exists():
        return []

    readings: list[Reading] = []
    for folder in sorted(folder_root.iterdir(), reverse=True):
        transcript = folder / TRANSCRIPT
        if not transcript.exists():
            continue
        text = transcript.read_text(encoding="utf-8")
        if query and query.lower() not in text.lower():
            continue
        readings.append(
            Reading(
                name=folder.name,
                title=title_of(folder),
                chunks=len(list(folder.glob("*.mp3"))),
                chars=len(text),
            )
        )
    return readings


def to_chunk(entry: dict[str, object]) -> Chunk:
    text = " ".join(str(entry["text"]).split())
    preview = text if len(text) <= PREVIEW_CHARS else text[:PREVIEW_CHARS].rsplit(" ", 1)[0] + "..."
    return Chunk(
        id=cast(int, entry["id"]),
        title=str(entry["title"]),
        chunk=f"{entry['index']}/{entry['of']}",
        state=str(entry["state"]),
        preview=preview,
    )


mcp = MCPServer(
    "spool",
    instructions=(
        "Spool reads text aloud on this Mac with OpenAI text to speech. spool_speak costs "
        "money per character, so replay saved readings with spool_replay instead of "
        "speaking the same text twice."
    ),
)

EFFECT = ToolAnnotations(
    read_only_hint=False, destructive_hint=False, idempotent_hint=False, open_world_hint=False
)
READ_ONLY = ToolAnnotations(
    read_only_hint=True, destructive_hint=False, idempotent_hint=True, open_world_hint=False
)


@mcp.tool(annotations=EFFECT)
def spool_speak(
    text: str, title: str = "", voice: str = "", instructions: str = "", position: Position = "last"
) -> Reply:
    """Read text aloud on the user's Mac through Spool.

    Starts Spool if it is not running, then returns as soon as the text is
    queued; speech carries on in the background in the same queue as the app's
    own hotkey. Send the words to be heard, not a file path. OpenAI charges per
    character, so for long texts check with the user first.

    Every reading is saved with its transcript, and `saved_as` in the reply
    names it. To hear it again, use spool_replay rather than a new spool_speak:
    a replay costs nothing.

    title: short name for the reading, shown in the app's queue. Always pass one.
    voice: alloy, ash, ballad, cedar, coral, echo, fable, marin, nova, onyx, sage,
    shimmer or verse. instructions: a tone prompt such as "calm and slow"
    (gpt-4o-mini-tts only). Leave both empty to use the app's settings.
    position: last (default) queues after everything, next plays when the
    current chunk ends, now plays at once and puts the interrupted chunk back
    at the top of up next.
    """
    if not text.strip():
        return Reply(ok=False, error="nothing to speak")

    folder = new_reading_folder(text, title)
    command: dict[str, object] = {
        "cmd": "speak",
        "text": text,
        "save_dir": str(folder),
        "title": title_of(folder),
        "at": position,
    }
    if voice:
        command["voice"] = voice
    if instructions:
        command["instructions"] = instructions

    # A reading the engine never took would show up in the library with no audio.
    try:
        reply = send(command)
    except ToolError:
        shutil.rmtree(folder)
        raise
    if not reply.get("ok"):
        shutil.rmtree(folder)
        return reply

    reply["saved_as"] = folder.name
    return reply


@mcp.tool(annotations=EFFECT)
def spool_control(action: Action, value: float = 0.0) -> Reply:
    """Control playback: pause, resume, move between chunks, stop, or read the state.

    play resumes, skip goes to the next chunk, previous to the one before,
    restart plays the current chunk from its start. stop drops everything up
    next and keeps what played; clear_played empties the played list. seek
    moves `value` seconds (negative goes back), speed sets playback speed to
    `value` (0.5 to 3), volume to `value` (0 to 1). status reports without
    changing anything and never starts Spool; quit stops the engine.

    In the reply, speaking means sound is coming out now, queued counts chunks
    left (loading ones are still being fetched), and stalled means it should be
    playing and is not, which Spool fixes itself within a few seconds.
    """
    command: dict[str, object] = {"cmd": action}
    if action == "seek":
        command["by"] = value
    elif action == "speed":
        command = {"cmd": "set_speed", "speed": value}
    elif action == "volume":
        command = {"cmd": "set_volume", "volume": value}

    # Asking a stopped engine for its state should answer, not start it.
    start_if_down = action not in ("status", "quit")
    return send(command, start_if_down=start_if_down)


@mcp.tool(annotations=EFFECT)
def spool_replay(
    name: str, from_chunk: int = 1, from_text: str = "", position: Position = "last"
) -> Reply:
    """Play a saved reading again, without paying for new audio.

    name: `saved_as` from spool_speak or a name from spool_library; part of a
    name works and picks the newest match. from_text starts at the chunk holding
    that phrase (chunks run a few sentences to a few paragraphs), from_chunk at
    that chunk number. position works as in spool_speak.
    """
    folder = find_reading(name)
    audio = sorted(folder.glob("*.mp3"))
    if not audio:
        return Reply(ok=False, error=f"{folder.name} has no saved audio")

    if from_chunk > len(audio):
        return Reply(ok=False, error=f"{folder.name} has {len(audio)} chunks, not {from_chunk}")
    start = max(from_chunk, 1) - 1
    if from_text:
        needle = from_text.lower()
        hits = [
            i for i, mp3 in enumerate(audio)
            if mp3.with_suffix(".txt").exists()
            and needle in mp3.with_suffix(".txt").read_text(encoding="utf-8").lower()
        ]
        if not hits:
            return Reply(ok=False, error=f"{from_text!r} is not in {folder.name}")
        start = hits[0]

    reply = send({
        "cmd": "play_files",
        "paths": [str(path) for path in audio[start:]],
        "title": title_of(folder) or folder.name,
        "at": position,
    })
    reply["saved_as"] = folder.name
    return reply


@mcp.tool(annotations=READ_ONLY)
def spool_queue() -> QueueView:
    """What is in Spool's queue right now, as the app shows it.

    current is the chunk playing, played holds the 10 most recent first
    (played_total counts them all), up_next is in play order. Each chunk has
    an `id` for spool_arrange, its reading's title, its place in that reading
    ("3/7") and its opening words.
    """
    reply = cast(dict[str, object], send({"cmd": "queue"}, start_if_down=False))
    if not reply.get("ok"):
        return QueueView(ok=False, error=str(reply.get("error", "no queue")))

    deck = cast(dict[str, object], reply["queue"])
    entries = [to_chunk(entry) for entry in cast(list[dict[str, object]], deck["entries"])]
    cursor = cast(int, deck["cursor"])
    played = entries[:cursor][::-1]
    recent_played = played[:PLAYED_SHOWN]
    current = entries[cursor] if cursor < len(entries) else None
    up_next = entries[cursor + 1:]

    return QueueView(
        ok=True,
        current=current,
        played=recent_played,
        played_total=len(played),
        up_next=up_next,
        paused=bool(reply.get("paused")),
        position=cast(float, reply.get("position", 0.0)),
        speed=cast(float, reply.get("speed", 1.0)),
    )


@mcp.tool(annotations=EFFECT)
def spool_arrange(action: Arrangement, chunk: int, before: int | None = None) -> Reply:
    """Rearrange the queue, as dragging and clicking rows in the app does.

    Take chunk ids from spool_queue. move puts `chunk` just before the chunk
    `before`, or at the end of up next when `before` is left out; a played
    chunk moved this way goes back into up next, and the chunk playing cannot
    be moved. jump plays from `chunk`. remove takes it out of the queue.
    """
    command: dict[str, object] = {"cmd": action, "id": chunk}
    if action == "move" and before is not None:
        command["before"] = before
    return send(command)


@mcp.tool(annotations=READ_ONLY)
def spool_library(query: str = "") -> list[Reading]:
    """Saved readings, newest first, with title, chunk count and length in characters.

    query keeps only readings whose title or text contain it. Use it before
    spool_speak when the text may have been read before, then spool_replay.
    """
    return list_readings(query)


if __name__ == "__main__":
    mcp.run()
