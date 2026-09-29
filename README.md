# logging.dll API Reference

**Product:** ThreeParams Logging
**Version:** 1.0.0
**Calling convention:** `system` (Windows stdcall on 32-bit; platform default on 64-bit)
**String encoding:** null-terminated C strings (`char*`). Message bytes are written exactly as passed in.

`logging.dll` appends lines to `.log` files without ever making the caller wait.
`LogText` puts the message on an in-memory queue and returns immediately. A
background thread writes each file's queued messages once it can open the file
**exclusively**, meaning nobody else has it open. If the file is open elsewhere,
the messages stay queued and the write is retried every 250 ms. Messages for
the same file are always written in the order they were logged, and a locked
file does not hold up writes to other files.

Build: `cargo build --release --target i686-pc-windows-msvc`

DataFlex bindings: [`dataflex/Logging.pkg`](dataflex/Logging.pkg)

---

## LogText

```c
long LogText(const char* psFilename, const char* psMessage);
```

Queues `psMessage` to be appended to `psFilename`. CRLF is appended unless the
message already ends in a newline. The file is created if it does not exist
(its folder must already exist).

| Returns | Meaning |
|---|---|
| `0` | Queued |
| `-1` | Null pointer, or the filename is not valid UTF-8 |
| `-2` | Filename does not end in `.log` (not case-sensitive) |

## LogStatus

```c
long LogStatus(void);
```

Returns the number of messages that have been queued but not yet written. A
number that stays above zero means a log file is being held open by another
program, or its path cannot be written (for example, the folder is missing).
Those messages are retried until they are written.

## LogFlush

```c
long LogFlush(long timeoutMs);
```

Waits up to `timeoutMs` milliseconds for the queue to empty, and returns the
number of messages still queued (`0` = everything written). The queue is held
only in memory, so **call this before the application exits**. Anything still
queued when the process ends is lost.
