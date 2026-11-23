"""What `cargo test` failed, as GitHub annotations: `python test_failures.py LOG`.

CI keeps the test step's output (tee) and runs this when the step fails:
each failing test becomes an `::error` with its name, the test binary, where
it panicked and the first lines of the panic's message, so a failure can be
read from the check run's annotations alone (the job's log is not always at
hand). A test binary that crashed (a signal, an access violation) and the
tests still running when the step ended are named too. The messages are
kept short: GitHub shows at most 10 errors a step and cuts long messages.

Always exits 0: it reports, the step's own exit status fails the job."""
import re
import sys

MAX_ERRORS = 9  # (the tenth: a summary of all the names)
MAX_MESSAGE = 900
MAX_LINES = 8

RUNNING = re.compile(r"^\s*(?:Running (?:unittests )?|Doc-tests )(\S+)")
SECTION = re.compile(r"^---- (\S+) stdout ----$")
PANIC = re.compile(r"panicked at (.+?):(\d+):(\d+):?$")
PANIC_OLD = re.compile(r"panicked at '(.*)', (.+?):(\d+):(\d+)$")
FAILED = re.compile(r"^test (\S+) \.\.\. FAILED$")
SLOW = re.compile(r"^test (\S+) has been running for over (\d+) seconds$")
CRASH = re.compile(r"(process didn't exit successfully: .*)$")
NOTE = re.compile(r"^note: run with `RUST_BACKTRACE")


def escape_data(text):
    return text.replace("%", "%25").replace("\r", "").replace("\n", "%0A")


def escape_property(text):
    return escape_data(text).replace(":", "%3A").replace(",", "%2C")


def annotation(kind, title, message, file=None, line=None):
    props = []
    if file:
        props.append("file=" + escape_property(file))
    if line:
        props.append("line=" + escape_property(line))
    props.append("title=" + escape_property(title[:200]))
    if len(message) > MAX_MESSAGE:
        message = message[: MAX_MESSAGE - 3] + "..."
    return f"::{kind} {','.join(props)}::{escape_data(message)}"


def parse(lines):
    """(failures, crashes, slow): failures as dicts of binary, name, file,
    line, message, in the order they failed."""
    failures = {}
    order = []
    crashes = []
    slow = {}
    binary = ""
    section = None
    body = []

    def close():
        nonlocal section, body
        if section is not None:
            key = (binary, section)
            entry = failures.setdefault(key, {"binary": binary, "name": section})
            if key not in order:
                order.append(key)
            entry.update(panic_of(body))
        section, body = None, []

    for raw in lines:
        line = raw.rstrip("\r\n")
        m = RUNNING.match(line)
        if m:
            close()
            binary = m.group(1).replace("\\", "/")
            continue
        m = SECTION.match(line)
        if m:
            close()
            section = m.group(1)
            continue
        if section is not None:
            if line in ("failures:", "successes:") or line.startswith("test result:"):
                close()
            else:
                body.append(line)
                continue
        m = FAILED.match(line)
        if m:
            key = (binary, m.group(1))
            if key not in failures:
                failures[key] = {"binary": binary, "name": m.group(1)}
                order.append(key)
            continue
        m = SLOW.match(line)
        if m:
            slow[(binary, m.group(1))] = m.group(2)
            continue
        m = CRASH.search(line)
        # (exit status 101: the binary's tests failed, named above)
        if m and not re.search(r"exit (?:status|code): 101\b", line):
            crashes.append((binary, m.group(1)))
    close()
    return [failures[k] for k in order], crashes, slow


def panic_of(body):
    """Where the section's (first) panic is and its message's first lines."""
    for i, line in enumerate(body):
        m = PANIC_OLD.search(line)
        if m:
            return {
                "file": m.group(2).replace("\\", "/"),
                "line": m.group(3),
                "message": m.group(1),
            }
        m = PANIC.search(line)
        if m:
            message = []
            for more in body[i + 1 :]:
                if NOTE.match(more) or PANIC.search(more):
                    break
                message.append(more)
                if len(message) >= MAX_LINES:
                    message.append("...")
                    break
            return {
                "file": m.group(1).replace("\\", "/"),
                "line": m.group(2),
                "message": "\n".join(message).strip() or line.strip(),
            }
    # (no panic: what the test printed last)
    tail = [l for l in body if l.strip()][-MAX_LINES:]
    return {"message": "\n".join(tail)}


def short(binary):
    # target/debug/deps/cover_skip-0123abcd -> cover_skip; src/lib.rs stays
    name = binary.strip("()").rsplit("/", 1)[-1]
    return re.sub(r"-[0-9a-f]{16}(\.exe)?$", "", name)


def report(lines):
    failures, crashes, slow = parse(lines)
    out = []
    for f in failures[:MAX_ERRORS]:
        title = f"{short(f['binary'])}: {f['name']}" if f["binary"] else f["name"]
        out.append(
            annotation(
                "error",
                title,
                f.get("message") or "failed (no message)",
                f.get("file"),
                f.get("line"),
            )
        )
    if len(failures) > MAX_ERRORS or (failures and crashes):
        names = ", ".join(f"{short(f['binary'])}::{f['name']}" for f in failures)
        out.append(annotation("error", f"{len(failures)} tests failed", names))
    for binary, what in crashes[:3]:
        out.append(annotation("error", f"{short(binary)}: crashed", what))
    for (binary, name), secs in list(slow.items())[:5]:
        if (binary, name) not in {(f["binary"], f["name"]) for f in failures}:
            out.append(
                annotation(
                    "warning",
                    f"{short(binary)}: {name}",
                    f"running for over {secs} seconds",
                )
            )
    if not failures and not crashes:
        out.append(
            annotation("error", "cargo test failed", "no failing test found in the output: a build error?")
        )
    return out


def main():
    with open(sys.argv[1], encoding="utf-8", errors="replace") as f:
        lines = f.readlines()
    for line in report(lines):
        print(line)


if __name__ == "__main__":
    main()
