# grove

A terminal dashboard of your git worktrees: which ones a Claude Code, Codex or
OpenCode session is working in, how much disk each one takes, and a key to
remove the ones you are done with, or all the idle ones at once, with
`git worktree remove`. Built for tiling window managers: resize the pane and
the layout follows.

```text
 grove   15 worktrees  ◆ 2 waiting for you  ⠸ 4 working  ● 3 idle  Σ 27.2 GB  disk █████▌·· 312 GB free

 board  ~/Workspace/board ───────────────────────────────────────────────────────────────────── 4 worktrees · 3.2 GB
 ● board main              ✓          ✻ claude  board-ui                                      1.4 GB ████▋·····    1h
 ⠸ archive-cards locked    ✓          ✻ claude  archive column cards · moving the column a…   612 MB ██········   now
 ● migrate-cards           ✓          ◈ opencode  pid 70001 · 0% cpu                          590 MB █▉········    2d
▍○ promote-subboard locked ✓                                                                  588 MB █▉········    6d

 shop  ~/Workspace/shop ────────────────────────────────────────────────────────────────────── 8 worktrees · 23.8 GB
 ◆ shop main               ✓          ✻ claude ×3  ◆1  ⠸1  ●1                                12.3 GB ██████████   now
 ◆ checkout-coupons        ±15 ↑3     ✻ claude  coupon rules · reply A or B: (A) apply the…   1.9 GB ██████▏···   now
 ⠸ order-pipeline          ±4 ↑1      ✻ claude  pipeline optimization · bug hunter scannin…   2.1 GB ██████▉···   now
 ◎ search-index            ✓          ◎ 3 Google Chrome                                       3.1 GB ██████████    1d
 ○ bench-cli               ✓                                                                  1.8 GB █████▊····   12d
 ○ api-error-messages      ✓                                                                  1.2 GB ███▉······    9d
 ○ fix-pr-512              ✓                                                                  905 MB ██▉·······    4d
 ○ cli-release             ±5                                                                 402 MB █▎········    3d

 tools  ~/Workspace/tools ───────────────────────────────────────────────────────────────────── 3 worktrees · 137 MB
 ○ tools main              ✓                                                                 96.0 MB ▎·········    1d
 ⠸ tools-dns               ✓          ❯ codex  pid 81234 · 38% cpu                           41.0 MB ▏·········   now
 ✗ tools-old-experiment               ✗ folder is gone                                             — ··········   41d

 ↑↓ move  ⏎ details  d remove  D remove idle  / filter  s sort: activity  r refresh  ? help  q quit
```

<details>
<summary>In a narrow tiling pane, with the details under the list</summary>

```text
 grove   15 wt  ◆2  ⠸4  ●3  27G  312 GB free

 board ────────────────────────── 4 · 3.2 GB ┃
 ● board main                          1.4 GB┃
   ✻ claude  board-ui                ███▊····┃
 ⠸ archive-cards locked                612 MB┃
   ✻ claude  archive column cards    █▋······┃
 ● migrate-cards                       590 MB┃
   ◈ opencode  pid 70001 · 0% cpu    █▌······┃
▍○ promote-subboard locked             588 MB┃
▍  ⎇ promote-subboard                █▌······┃
                                             ┃
 shop ────────────────────────── 8 · 23.8 GB │
 ◆ shop main                          12.3 GB│
   ✻ claude ×3  ◆1  ⠸1  ●1           ████████│
 ◆ checkout-coupons                    1.9 GB│
   ✻ claude  coupon rules            █████···│
 ⠸ order-pipeline                      2.1 GB│
   ✻ claude  pipeline optimization   █████▌··│
 ◎ search-index                        3.1 GB│
   ◎ 3 Google Chrome                 ████████│
╭ promote-subboard ────────────────── ○ free ╮
│ ~/Workspace/b…e/worktrees/promote-subboard │
│ ⎇ promote-subboard                         │
│ 903109d chore: sync · 1 day ago            │
│ ✓ clean                                    │
│ ⊘ locked: claude session promote-subboard  │
│ (pid 11111 start Sat Sep 19 09:02:11 2026… │
│ last used 6 days ago                       │
│                                            │
│ SESSIONS                                   │
│ No agent session here                      │
╰────────────────────────────────────────────╯
 ↑↓ move  ⏎ details  d remove  D remove idle
```

</details>

<details>
<summary>Removing a worktree an agent still works in (<code>d</code>)</summary>

```text
╭ Remove worktree ───────────────────────────────────────────────────────────────────────╮
│ checkout-coupons  1.9 GB                                                               │
│ ~/Workspace/shop/.claude/worktrees/checkout-coupons                                    │
│                                                                                        │
│ ■ Stops the process running in it:                                                     │
│     ✻ claude  coupon rules · pid 58380                                                 │
│ ± 15 changed or untracked files will be lost                                           │
│ · Branch worktree-checkout-coupons stays; delete it with git branch -d                 │
│   worktree-checkout-coupons                                                            │
│                                                                                        │
│ $ git -C ~/Workspace/shop worktree remove --force                                      │
│ ~/Workspace/shop/.claude/worktrees/checkout-coupons                                    │
│                                                                                        │
│  ⏎  remove     Esc  cancel                                                             │
╰────────────────────────────────────────────────────────────────────────────────────────╯
```

</details>

<details>
<summary>Removing every idle worktree (<code>D</code>)</summary>

```text
╭ Remove idle worktrees ─────────────────────────────────────────────────────────────────╮
│ 6 worktrees with no agent session and no process · 4.9 GB                              │
│                                                                                        │
│ ○ shop / bench-cli                                                              1.8 GB │
│ ○ shop / api-error-messages                                                     1.2 GB │
│ ○ shop / fix-pr-512                                                             905 MB │
│ ○ board / promote-subboard                                                      588 MB │
│ ○ shop / cli-release                                                            402 MB │
│ ○ tools / tools-old-experiment                                                       — │
│                                                                                        │
│ ± 1 of them has uncommitted files, which will be lost                                  │
│ $ git worktree remove --force on each; the main worktrees stay                         │
│                                                                                        │
│  ⏎  remove all     Esc  cancel                                                         │
╰────────────────────────────────────────────────────────────────────────────────────────╯
```

</details>

On a real terminal the states have colors (pink for a session waiting for
you, green for one at work, amber for an idle one), each agent has its own,
and the size bars run from teal for the small worktrees to red for the
largest.

## Features

- **Every worktree on the computer, grouped by repository.** grove looks for
  repositories in your code folders (`~/Workspace`, `~/Projects`, `~/src`,
  `~/code` and the like, or the folders you pass), in the projects Claude
  Code and Codex remember, and wherever an agent session is running. Every
  repository with linked worktrees is listed, main worktree first.
- **Which agent is in which worktree.** Claude Code, Codex and OpenCode
  sessions are found by their processes and working folders. For Claude
  Code, grove also shows the session name, whether it is working or waiting
  for you, and what it says it is doing. A session that runs something in
  another worktree (tests, a server) shows up there too, as "via ruby".
  Other processes with their folder in a worktree (a shell, a dev server, a
  browser) are listed as well.
- **How much space each one takes.** Worktrees are measured like `du` does
  (allocated blocks, hard links once, nested worktrees left out) in the
  background, while you look. Bars compare them, the details list the
  biggest folders inside, and the header shows the total and the free space
  on the disk.
- **Removal in one step.** `d` asks once and removes: it stops whatever runs
  in the worktree (agent sessions, servers, shells), then runs
  `git worktree remove --force`, twice forced for a locked worktree, as git
  asks. The question shows the exact command, what will be stopped, the
  uncommitted files that will be lost, the lock and the branch that stays.
  The removal runs in the background; the list stays in your hands.
- **Remove every idle worktree at once.** `D` removes all the worktrees on
  the list with no agent session and no process running, after one
  question that lists them with their sizes. With a filter on, only the
  ones it shows. A main worktree is never removed.
- **Responsive.** A table with a details panel beside it on a wide screen,
  the details under the list in a tall pane, two-line cards in a narrow one,
  and a line per worktree with just its name and size in a tiny one. Columns
  come and go with the width. Key hints and dialog buttons take a click.
- **English or Portuguese**, picked from `$LANG` (or `--lang`).

## Install

```sh
cargo install --git https://github.com/victorlcampos/grove
```

Needs Rust 1.88 or newer ([rustup.rs](https://rustup.rs)) and git.

## Usage

```sh
grove                  # your code folders
grove ~/code ~/src     # other folders
grove --depth 5        # look deeper for repositories (default 3 levels)
grove --no-mouse       # leave the mouse to the terminal, to select text
```

### Keys

| Key | Action |
| --- | --- |
| `↑` `↓` or `j` `k` | Move (also the mouse wheel and a click) |
| `PgUp` `PgDn`, `g` `G` | Page, first, last |
| `Enter` | Show or hide the details (full screen when the window is small) |
| `d` | Remove the worktree, stopping what runs in it |
| `D` | Remove every idle worktree on the list |
| `/` | Filter by name, branch, path or session name |
| `s` | Sort by activity, size, name or oldest |
| `r` | Refresh now and measure the selected worktree again |
| `R` | Measure every worktree again |
| `?` | Help |
| `q` or `Ctrl+C` | Quit |

In a removal question, `Enter` removes and `Esc` cancels.

### States

| Symbol | Meaning |
| --- | --- |
| `◆` | An agent session there waits for you |
| `⠸` | An agent session is working there |
| `●` | An agent session is open there, idle |
| `◎` | No agent, but some process runs there |
| `○` | Free: what `D` removes |
| `✗` | The folder is gone; removing only clears git's record |

`±12` counts changed and untracked files, `↑3 ↓1` commits ahead and behind
the upstream; `✻` is Claude Code, `❯` Codex and `◈` OpenCode.

## How it knows

- **Worktrees** come from `git worktree list --porcelain`, and their state
  from `git status`. grove runs git with `GIT_OPTIONAL_LOCKS=0`, so it never
  takes the index lock from an agent committing at the same moment.
- **Sessions** come from the process list: an agent process whose working
  folder is inside a worktree is working there. Claude Code describes its
  sessions in `~/.claude/sessions/<pid>.json` and its background jobs in
  `~/.claude/jobs/<id>/state.json`, which is where the names, states and
  "waiting for you" come from (`CLAUDE_CONFIG_DIR` and `CODEX_HOME` are
  honored). Commands an agent runs count through the shell running them;
  helpers it starts on its own, like MCP servers, stay in the folder the
  session started in and are left out.
- **Removing** stops every process working inside the worktree, helpers
  included: it asks them to quit, and kills the ones still there after three
  seconds. Each process is checked again right before, so a number taken by
  another program since the last look is left alone, and grove never stops
  itself or the terminal it runs in.
- **Sizes** are measured once when a worktree shows up and again every hour,
  or every five minutes while an agent works in it.
- **On macOS**, grove stays out of the folders the system guards with a
  permission prompt (Desktop, Documents, Downloads, Library and removable
  volumes) unless you pass them on the command line.
