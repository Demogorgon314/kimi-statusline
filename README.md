# kimi-statusline

用 Rust 写的 [Kimi Code CLI](https://github.com/MoonshotAI/kimi-code) 底部状态栏：整会话 token 用量与缓存命中率、5 小时 / 7 天额度与重置时间、子 agent 用量、git 与 PR、10 套主题，以及 [CCometixLine](https://github.com/Haleclipse/CCometixLine) 风格的 TUI 配置器。数据解析参考 [kimi-usage](https://github.com/YD-233/kimi-usage)。

![kimi-statusline 在 Kimi Code 中的效果](assets/hero.png)

需要 kimi-code ≥ 0.30.0（支持 `[status_line].command`）。

## 安装

### 方式一：一键安装（推荐）

macOS / Linux：

```bash
curl -fsSL https://raw.githubusercontent.com/Demogorgon314/kimi-statusline/main/install.sh | sh
```

Windows（PowerShell）：

```powershell
irm https://raw.githubusercontent.com/Demogorgon314/kimi-statusline/main/install.ps1 | iex
```

脚本从 GitHub Release 下载对应平台的预编译二进制，放到 `~/.local/bin`，并写入 `~/.kimi-code/tui.toml` 的 `[status_line].command`。然后在 Kimi Code 里执行 `/reload-tui`。

- 指定版本：`KIMI_STATUSLINE_VERSION=v0.1.0`；指定安装目录：`KIMI_STATUSLINE_BIN_DIR=...`
- `tui.toml` 里已有别的 `command` 时不会覆盖，按提示执行 `kimi-statusline install --force`
- 改写前会检查结果是合法 TOML，注释原样保留，原文件备份为 `tui.toml.bak`

### 方式二：作为 Kimi Code 插件安装

在 Kimi Code 里执行：

```
/plugins install https://github.com/Demogorgon314/kimi-statusline
/reload
/new
/reload-tui
```

插件的 `SessionStart` hook 会自动下载与插件版本对应的预编译二进制（只在第一次），并把 `[status_line].command` 指向它。`/new` 触发 hook，`/reload-tui` 让状态栏生效。

- `/plugins disable` 或 `/plugins remove` 之后，状态栏会自己从 `tui.toml` 里删掉这一项，并在 `/reload-tui` 之前显示内置样式，不会卡在旧内容上
- 下载失败时，会退回到 `PATH` 上已有的 `kimi-statusline`，或（装了 Rust 时）在后台从源码编译，下一个会话生效

### 方式三：从源码安装

```bash
cargo install --git https://github.com/Demogorgon314/kimi-statusline
kimi-statusline install
```

### 卸载

```bash
kimi-statusline uninstall          # 删除 tui.toml 里的 command，然后 /reload-tui
rm -rf ~/.kimi-code/kimi-statusline ~/.kimi-code/kimi-statusline-cache
```

插件方式安装的，在 Kimi Code 里执行 `/plugins remove kimi-statusline`。

## 段（segments）

| id | 内容 |
| --- | --- |
| `mode` | 权限模式 / plan / swarm / tower 徽标 |
| `goal` | `[goal ● active · 4m · 7 turns]` |
| `model` | 模型显示名与思考强度（跟随 `/effort`）；`/dance` 后显示彩虹，`/dance on` 定格 |
| `tasks` | `[2 tasks running]` / `[1 agent running]` |
| `directory` | 工作目录（`depth` 控制保留几级） |
| `git` | 分支、diff 统计、ahead/behind，以及当前分支打开的 PR（可点击的 `[PR#42]`，需安装并登录 `gh`） |
| `context` | 上下文窗口占用（`kimi` 主题默认关闭：内置状态栏第二行已经显示） |
| `usage` | 整个会话（主 agent + 全部子 agent）的总输入 ↑、总输出 ↓、缓存命中率（低红高绿渐变） |
| `subagent` | 用量最多的子 agent 模型的同样三项 |
| `session` | 会话已持续的时间（默认关闭） |
| `quota` | 套餐额度：5 小时 / 7 天（可选月度）已用百分比和重置时间，如 `5h 42% ↻1h20m · 7d 13% ↻Sun 08:00`；需要 Kimi Code OAuth 登录 |

![自适应宽度](assets/adaptive.png)

终端变窄时会先切换成紧凑写法，再依次去掉 session → git → directory → subagent → tasks → goal → context → quota → mode，实在放不下才用省略号截断。

### 额度（quota）

数据来自 Kimi Code `/usage` 命令用的同一个接口（`GET <base>/usages`），用的是 Kimi Code 存在 `~/.kimi-code/credentials/` 里的登录凭据。

- 网络请求放在后台进程里做（默认每 120 秒最多一次，`refresh_secs` 可调），状态栏本身只读缓存，不会变慢
- 重置时间在 24 小时内显示倒计时（`↻1h20m`），更远的显示本地星期和时间（`↻Sun 08:00`）
- 已用比例 ≥50% 变黄、≥85% 变红；`bar = true` 时显示进度条
- 本工具**不会**刷新登录凭据（Kimi Code 刷新时会轮换 refresh token，抢着刷新可能把你登出）。Kimi Code 闲置超过约 15 分钟后凭据过期，额度会停在最后一次拿到的数值，下次在 Kimi Code 里发消息后自动恢复
- `kimi-statusline quota` 立即拉取一次并打印，可用来排查问题

## 主题与配置

![内置主题](assets/themes.png)

内置主题：`kimi`（默认，外观与内置状态栏一致，颜色跟随 TUI 主题）、`cometix`、`default`、`minimal`、`gruvbox`、`nord`、`powerline-dark`、`powerline-light`、`powerline-rose-pine`、`powerline-tokyo-night`。`kimi-statusline themes` 列出全部主题。

```bash
kimi-statusline                      # 主菜单：配置、初始化、检查配置、安装/卸载、测试额度
kimi-statusline config               # 直接进入配置器
kimi-statusline init -t nord         # 用某个主题生成配置文件
kimi-statusline -t powerline-dark preview
```

![TUI 配置器](assets/configurator.png)

配置器的布局和按键与 CCometixLine 一致：顶部是实时预览和主题栏，左边是段列表，右边是所选段的设置。

| 按键 | 作用 |
| --- | --- |
| `Tab` | 在段列表和设置面板之间切换 |
| `↑↓` | 选择 |
| `Enter` / 空格（段列表） | 显示 / 隐藏该段 |
| `Shift+↑↓` 或 `J`/`K` | 调整段的顺序 |
| `Enter`（设置面板） | 编辑：颜色选择器（Kimi 配色 / 16 色 / 256 色 / RGB）、图标选择器、选项值 |
| `←→`（设置面板） | 切换开关、调整数值 |
| `1`-`9` / `P` | 选择 / 切换主题 |
| `M` | plain → nerd_font → powerline |
| `E` | 分隔符（预设或自定义） |
| `L` / `C` | 中英文 / 配色来源 |
| `R` | 重置为当前主题默认值 |
| `S` / `W` / `Ctrl+S` | 保存配置 / 写回当前主题 / 另存为新主题 |
| `?` / `Esc` | 帮助 / 退出 |

预览用当前目录最新的会话数据，缺的部分用示例数据补上，方便看到每个段的样子。

配置文件在 `~/.kimi-code/kimi-statusline/config.toml`，自定义主题在 `~/.kimi-code/kimi-statusline/themes/<name>.toml`，格式与 CCometixLine 相同：

```toml
theme = "kimi"

[style]
mode = "plain"          # plain | nerd_font | powerline
separator = "  "        # "" 为 powerline 箭头
lang = "en"             # zh 显示 总计 / 缓存 / 上下文
palette = ""            # "" 跟随 tui.toml；或 dark / light / Kimi 自定义主题名
width = 0               # 0 = 自动检测终端宽度

[[segments]]
id = "git"
enabled = true
icon = { plain = "🌿", nerd_font = "\U000f02a2" }
colors = { text = "text_dim" }   # 也可以是 { c16 = 12 }、{ c256 = 109 }、{ r = 1, g = 2, b = 3 }、"#rrggbb"
styles = { text_bold = false }
options = { status = true, pr = true, pr_link = true }
```

颜色除了 16 色、256 色和 RGB，还可以写 Kimi 配色名（`text`、`primary`、`accent`、`text_dim`、`text_muted`、`success`、`warning`、`error`），会跟随 TUI 的 dark / light / 自定义主题变化。

各段的选项：`quota.show_5h` / `show_7d` / `show_month` / `show_reset` / `bar` / `colorful` / `refresh_secs`、`model.dance`、`directory.depth`、`git.status` / `git.pr` / `git.pr_link`、`context.show_tokens` / `context.colorful`、`usage.colorful` / `usage.show_cache`、`subagent.colorful`。

## 性能

TUI 每秒最多执行一次状态栏命令，超过 300ms 就杀掉。本工具：

- 增量读取 `wire.jsonl`：缓存每个文件读到的位置，每次只解析新追加的部分；超大会话的首次解析会分摊到多次刷新里，不会超时
- git 状态缓存 15 秒，后台任务计数缓存 2 秒
- `gh pr view` 在后台独立进程里运行（每分钟最多一次），结果下次刷新再用，不会拖慢渲染
- 终端宽度通过系统调用找到 TUI 所在的 tty（命令本身是脱离终端启动的），不调用 `ps`

热缓存下一次运行约 10–20ms，缓存全空时约 60ms。

## 调试

```bash
kimi-statusline preview --cwd ~/proj --width 80
kimi-statusline quota                         # 立即拉取并打印额度
```

设置 `KIMI_STATUSLINE_DEBUG=1`，或 `touch ~/.kimi-code/kimi-statusline-debug`（即时生效），日志写到 `~/.kimi-code/kimi-statusline-debug.log`。`KIMI_STATUSLINE_NO_COLOR=1` 输出纯文本。缓存在 `~/.kimi-code/kimi-statusline-cache/`，可随时删除。

## 开发

```bash
cargo test
python3 scripts/screenshots.py   # 重新生成 assets/ 下的截图（需要 Chrome、ImageMagick、Nerd Font；配置器截图还需要 pip install pyte）
```

发布：同步修改 `Cargo.toml` 和 `kimi.plugin.json` 里的版本号，然后推送 `vX.Y.Z` tag，GitHub Actions 会编译 6 个平台并创建 Release。

## License

MIT
