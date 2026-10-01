# kimi-statusline

English | [中文](README.zh.md)

A fast, themeable status line for [Kimi Code CLI](https://github.com/MoonshotAI/kimi-code), written in Rust. It shows whole-session token usage and cache hit rate, your 5-hour / 7-day plan quota with reset times, sub-agent usage, git and PR status, and comes with 10 themes and a [CCometixLine](https://github.com/Haleclipse/CCometixLine)-style TUI configurator. Session parsing follows [kimi-usage](https://github.com/YD-233/kimi-usage).

![kimi-statusline in Kimi Code](assets/hero.png)

Requires kimi-code ≥ 0.30.0 (for `[status_line].command`).

## Install

### Option 1: one-line install (recommended)

macOS / Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/Demogorgon314/kimi-statusline/main/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/Demogorgon314/kimi-statusline/main/install.ps1 | iex
```

The script downloads the prebuilt binary for your platform from GitHub Releases into `~/.local/bin` and sets `[status_line].command` in `~/.kimi-code/tui.toml`. Then run `/reload-tui` in Kimi Code.

- Pin a version with `KIMI_STATUSLINE_VERSION=v0.1.0`; choose the install directory with `KIMI_STATUSLINE_BIN_DIR=...`
- An existing `command` that isn't ours is never overwritten; run `kimi-statusline install --force` to replace it
- The edited `tui.toml` is validated before it is written, comments are kept, and the previous file is saved as `tui.toml.bak`

### Option 2: as a Kimi Code plugin

In Kimi Code:

```
/plugins install https://github.com/Demogorgon314/kimi-statusline
/reload
/new
/reload-tui
```

The plugin's `SessionStart` hook downloads the prebuilt binary matching the plugin version (first run only) and points `[status_line].command` at it. `/new` fires the hook; `/reload-tui` picks up the new status line.

- After `/plugins disable` or `/plugins remove`, the status line removes itself from `tui.toml` and renders the built-in look until `/reload-tui`, so it never freezes on stale content
- If the download fails, the hook falls back to a `kimi-statusline` already on `PATH`, or (with Rust installed) builds from source in the background for the next session

### Option 3: from source

```bash
cargo install --git https://github.com/Demogorgon314/kimi-statusline
kimi-statusline install
```

### Uninstall

```bash
kimi-statusline uninstall          # removes our command from tui.toml; then /reload-tui
rm -rf ~/.kimi-code/kimi-statusline ~/.kimi-code/kimi-statusline-cache
```

For the plugin install, run `/plugins remove kimi-statusline` in Kimi Code.

## Segments

| id | Shows |
| --- | --- |
| `mode` | Permission mode / plan / swarm / tower badges |
| `goal` | `[goal ● active · 4m · 7 turns]` |
| `model` | Model display name and thinking effort (follows `/effort`); rainbow after `/dance`, frozen with `/dance on` |
| `tasks` | `[2 tasks running]` / `[1 agent running]` |
| `directory` | Working directory (`depth` sets how many path segments are kept) |
| `git` | Branch, diff stats, ahead/behind, and the branch's open PR as a clickable `[PR#42]` (needs `gh`, logged in) |
| `context` | Context window fill (off in the `kimi` theme: the built-in footer's second line already shows it) |
| `usage` | Whole-session (main + all sub-agents) input ↑, output ↓ and cache hit rate, on a red-to-green ramp |
| `subagent` | The same three numbers for the heaviest sub-agent model |
| `session` | Time since the session started (off by default) |
| `quota` | Plan quota: 5-hour / 7-day (optionally monthly) usage with reset times, e.g. `5h 42% ↻1h20m · 7d 13% ↻Sun 08:00`; needs a Kimi Code OAuth login |

![Adaptive width](assets/adaptive.png)

When the terminal gets narrow, the line first switches to compact labels, then drops segments in this order: session → git → directory → subagent → tasks → goal → context → quota → mode. Only as a last resort is it cut with an ellipsis.

### Quota

The numbers come from the same endpoint Kimi Code's `/usage` command uses (`GET <base>/usages`), authenticated with the login Kimi Code stores in `~/.kimi-code/credentials/`.

- The request runs in a background process (at most every 120 s, adjustable with `refresh_secs`); the status line itself only reads a cache and never slows down
- Resets within 24 hours show as a countdown (`↻1h20m`), later ones as local weekday and time (`↻Sun 08:00`)
- Usage turns yellow at ≥50% and red at ≥85%; `bar = true` adds a progress bar
- kimi-statusline **never** refreshes your login. Kimi Code rotates the refresh token when it refreshes, and racing it could log you out. When Kimi Code sits idle for about 15 minutes the access token expires and the quota holds its last value; it updates again after your next message in Kimi Code
- `kimi-statusline quota` fetches and prints the quota immediately, useful for troubleshooting

## Themes and configuration

![Built-in themes](assets/themes.png)

Built-in themes: `kimi` (default; matches the built-in footer and follows the TUI theme), `cometix`, `default`, `minimal`, `gruvbox`, `nord`, `powerline-dark`, `powerline-light`, `powerline-rose-pine`, `powerline-tokyo-night`. `kimi-statusline themes` lists them all.

```bash
kimi-statusline                      # main menu: configure, init, check config, install/uninstall, test quota
kimi-statusline config               # straight to the configurator
kimi-statusline init -t nord         # write a config file from a theme
kimi-statusline -t powerline-dark preview
```

![TUI configurator](assets/configurator.png)

The configurator's layout and keys follow CCometixLine: live preview and theme bar on top, the segment list on the left, the selected segment's settings on the right.

| Key | Action |
| --- | --- |
| `Tab` | Switch between the segment list and the settings panel |
| `↑↓` | Select |
| `Enter` / `Space` (segment list) | Show / hide the segment |
| `Shift+↑↓` or `J`/`K` | Reorder segments |
| `Enter` (settings) | Edit: color picker (Kimi palette / 16 / 256 / RGB), icon picker, option values |
| `←→` (settings) | Flip switches, adjust numbers |
| `1`-`9` / `P` | Pick / cycle theme |
| `M` | plain → nerd_font → powerline |
| `E` | Separator (presets or custom) |
| `L` / `C` | Language / color source |
| `R` | Reset to the current theme's defaults |
| `S` / `W` / `Ctrl+S` | Save config / write back to the current theme / save as a new theme |
| `?` / `Esc` | Help / quit |

The preview uses the newest session in the current directory and fills in sample data for anything missing, so every segment is visible.

The config lives in `~/.kimi-code/kimi-statusline/config.toml`, custom themes in `~/.kimi-code/kimi-statusline/themes/<name>.toml`, in the same format as CCometixLine:

```toml
theme = "kimi"

[style]
mode = "plain"          # plain | nerd_font | powerline
separator = "  "        # "" for powerline arrows
lang = "en"             # zh: 总计 / 缓存 / 上下文 labels
palette = ""            # "" follows tui.toml; or dark / light / a Kimi custom theme name
width = 0               # 0 = detect the terminal width

[[segments]]
id = "git"
enabled = true
icon = { plain = "🌿", nerd_font = "\U000f02a2" }
colors = { text = "text_dim" }   # also { c16 = 12 }, { c256 = 109 }, { r = 1, g = 2, b = 3 }, "#rrggbb"
styles = { text_bold = false }
options = { status = true, pr = true, pr_link = true }
```

Besides 16-color, 256-color and RGB, a color can be a Kimi palette name (`text`, `primary`, `accent`, `text_dim`, `text_muted`, `success`, `warning`, `error`) that follows the TUI's dark / light / custom theme.

Per-segment options: `quota.show_5h` / `show_7d` / `show_month` / `show_reset` / `bar` / `colorful` / `refresh_secs`, `model.dance`, `directory.depth`, `git.status` / `git.pr` / `git.pr_link`, `context.show_tokens` / `context.colorful`, `usage.colorful` / `usage.show_cache`, `subagent.colorful`.

## Performance

The TUI runs the status line command at most once a second and kills it after 300 ms. kimi-statusline:

- Reads `wire.jsonl` incrementally: it caches each file's read position and only parses what was appended. The first pass over a huge session is spread across several refreshes, so it never times out
- Caches git status for 15 s and background task counts for 2 s
- Runs `gh pr view` in a detached process (at most once a minute) and uses the result on a later refresh, so rendering never waits on it
- Finds the TUI's terminal width through system calls (the command itself runs detached from the terminal), without spawning `ps`

A run takes about 10–20 ms with warm caches and about 60 ms cold.

## Debugging

```bash
kimi-statusline preview --cwd ~/proj --width 80
kimi-statusline quota                         # fetch and print quota now
```

Set `KIMI_STATUSLINE_DEBUG=1`, or `touch ~/.kimi-code/kimi-statusline-debug` (takes effect immediately), to log to `~/.kimi-code/kimi-statusline-debug.log`. `KIMI_STATUSLINE_NO_COLOR=1` prints plain text. Caches live in `~/.kimi-code/kimi-statusline-cache/` and can be deleted at any time.

## Development

```bash
cargo test
python3 scripts/screenshots.py   # regenerate the images in assets/
```

The screenshot script needs Chrome, ImageMagick and a Nerd Font; the configurator shot also needs `pip install pyte`.

Releasing: bump the version in both `Cargo.toml` and `kimi.plugin.json`, then push a `vX.Y.Z` tag. GitHub Actions builds 6 platforms and creates the Release.

## License

MIT
