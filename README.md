# brew

A terminal email client for Maildir folders, written in Rust.

## Features

- Threaded email list with tree indentation
- Diff/patch syntax highlighting in email bodies
- Reply-all composition with quoting in `$VISUAL`/`$EDITOR`
- Compose new emails from scratch
- Draft saving to a dedicated Drafts mailbox
- Read/unread tracking via Maildir filename flags
- Multiple mailboxes with sidebar
- Tab-based email viewing
- Live search filtering by subject
- Unread-only filter toggle
- Sort threads by most-recent activity (ascending / descending)
- SMTP sending via STARTTLS
- Periodic background sync via a configurable shell command (e.g. `mbsync`)

## Configuration

Create `~/.config/brew/config.toml`:

```toml
[smtp]
host     = "smtp.gmail.com"
port     = 587
name     = "Name Surname"
username = "you@example.com"
password = "app-password"

[[mailbox]]
label = "INBOX"
path  = "/home/you/Mail/INBOX/"

[[mailbox]]
label = "Work"
path  = "/home/you/Mail/Work/"

# Optional: a mailbox labelled "Drafts" is treated specially.
# Pressing Enter on a draft reopens it in the editor.
# The send dialog offers [d] to save there.
[[mailbox]]
label = "Drafts"
path  = "/home/you/Mail/Drafts/"

# Optional: run a shell command every N seconds to sync mail.
# Errors are shown in the status bar; the UI is never blocked.
[sync]
command  = "mbsync -a"
interval = 60
```

Optional files (plain text, loaded from `~/.config/brew/`):

| File        | Purpose                 |
| ----------- | ----------------------- |
| `signature` | Appended to every draft |

## Key bindings

### Thread list

| Key                   | Action                                                     |
| --------------------- | ---------------------------------------------------------- |
| `j` / `k` / `↑` / `↓` | Move down / up                                             |
| `g`                   | Jump to first email                                        |
| `G`                   | Jump to last email                                         |
| `J` / `K`             | Next / previous mailbox                                    |
| `Enter`               | Open email in a new tab (Drafts mailbox: reopen in editor) |
| `r`                   | Reply-all (empty body)                                     |
| `R`                   | Reply-all (quoted)                                         |
| `C`                   | Compose new email                                          |
| `v`                   | Toggle read / unread                                       |
| `m`                   | Move email to another mailbox (popup picker)               |
| `s`                   | Toggle sort order (descending / ascending)                 |
| `N`                   | Toggle unread-only filter                                  |
| `/`                   | Search by subject (live)                                   |
| `Esc`                 | Clear search filter                                        |
| `f`                   | Force sync mailboxes                                       |
| `D`                   | Delete email                                               |
| `Q`                   | Quit                                                       |

### Email tab

| Key                   | Action                           |
| --------------------- | -------------------------------- |
| `j` / `k` / `↑` / `↓` | Scroll down / up one line        |
| `Ctrl+D` / `PageDown` | Scroll 15 lines down             |
| `Ctrl+U` / `PageUp`   | Scroll 15 lines up               |
| `Y`                   | Copy email body in the clipboard |
| `g`                   | Top of body                      |
| `G`                   | Bottom of body                   |
| `J` / `K`             | Next / previous email            |
| `r`                   | Reply (empty body)               |
| `R`                   | Reply (quoted)                   |
| `m`                   | Move email to another mailbox    |
| `D`                   | Delete email and close tab       |
| `q`                   | Close tab                        |

### Compose editor

| Key | Action                           |
| --- | -------------------------------- |
| `q` | Exit editor and show send dialog |
| `y` | Send (in send dialog)            |
| `d` | Save to Drafts (in send dialog)  |
| `n` | Discard (in send dialog)         |

### Global

| Key      | Action              |
| -------- | ------------------- |
| `Ctrl+N` | Next tab (circular) |
| `Ctrl+P` | Prev tab (circular) |
