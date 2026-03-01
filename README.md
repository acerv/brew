# brew

A terminal email client for Maildir folders, written in Rust.

## Features

- Threaded email list with tree indentation
- Diff/patch syntax highlighting in email bodies
- Reply-all and blank-reply composition in `$VISUAL`/`$EDITOR`
- Pre-written "thanks" replies
- Read/unread tracking via Maildir filename flags (no external state file)
- Filesystem watcher — thread list refreshes automatically on new mail
- Multiple mailboxes, switchable from the list view
- SMTP sending via STARTTLS (Gmail App Passwords supported)

## Configuration

Create `~/.config/brew/config.toml`:

```toml
[smtp]
host     = "smtp.gmail.com"
port     = 587
username = "you@example.com"
password = "app-password"

[[mailbox]]
label = "INBOX"
path  = "/home/you/Mail/INBOX/"

[[mailbox]]
label = "Work"
path  = "/home/you/Mail/Work/"
```

Optional files (plain text, loaded from `~/.config/brew/`):

| File        | Purpose                                  |
| ----------- | ---------------------------------------- |
| `signature` | Appended to every reply draft            |
| `thanks`    | Body pre-filled for `t` (thanks) replies |

## Key bindings

### List view

| Key                   | Action                  |
| --------------------- | ----------------------- |
| `j` / `k`             | Move down / up          |
| `Home`                | Jump to top             |
| `End`                 | Jump to bottom          |
| `PageDown` / `Ctrl+D` | Skip 15 down            |
| `PageUp` / `Ctrl+U`   | Skip 15 up              |
| `J` / `K`             | Next / previous mailbox |
| `Enter`               | Open email              |
| `r`                   | Reply-all (quoted)      |
| `R`                   | Reply-all (blank)       |
| `t`                   | Send thanks reply       |
| `N`                   | Show unread only        |
| `n`                   | Show all emails         |
| `D`                   | Delete email            |
| `h` / `l`             | Switch tab left / right |
| `q` / `Esc`           | Quit                    |

### Email view

| Key                   | Action                     |
| --------------------- | -------------------------- |
| `j` / `k`             | Scroll down / up one line  |
| `PageDown` / `Ctrl+D` | Scroll 15 lines down       |
| `PageUp` / `Ctrl+U`   | Scroll 15 lines up         |
| `g` / `Home`          | Top of body                |
| `G`                   | Bottom of body             |
| `J` / `K`             | Next / previous thread     |
| `r`                   | Reply-all (quoted)         |
| `R`                   | Reply-all (blank)          |
| `t`                   | Send thanks reply          |
| `D`                   | Close tab and delete email |
| `h` / `l`             | Switch tab left / right    |
| `Esc`                 | Back to list               |
| `q`                   | Close tab                  |

### Global

| Key      | Action              |
| -------- | ------------------- |
| `Ctrl+N` | Next tab (circular) |
| `Ctrl+P` | Prev tab (circular) |
