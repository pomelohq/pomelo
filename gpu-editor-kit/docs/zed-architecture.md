# Zed architecture map + Rust migration plan

Reference for growing `pomelo-editor-kit` into a full, cross-platform (Linux/macOS/Windows)
Rust application that replaces the Swift/AppKit shell. Studied from the Zed clone in
`../zed`. Goal: migrate the desktop app Swift -> Rust incrementally, porting Editor, Pane,
Terminal and File tree.

## Target subsystems (what to port) and where they live in Zed

| Subsystem | Zed crates | Core types | Per-frame flow |
|---|---|---|---|
| Editor | `editor` (editor.rs, element.rs, display_map.rs + fold/wrap/tab/block/inlay maps, selections_collection.rs, scroll.rs, movement.rs, blink_manager.rs), `text`, `rope`, `multi_buffer`, `language` | `Editor`, `EditorElement`, `DisplayMap`, `SelectionsCollection`, `ScrollManager`, `Anchor`, `Rope` | request_layout -> prepaint (snapshot, visible-row range, line layouts) -> paint 8 layers (bg, indent guides, line#/gutter, text, highlights, cursors, scrollbars, overlays) |
| Pane / Workspace | `workspace` (workspace.rs, pane.rs, pane_group.rs, dock.rs, item.rs) | `Workspace`, `Pane`, `PaneGroup` (`Member = Pane\|Axis`, `flexes`), `Dock`, `Item` trait | Workspace.render -> PaneGroup recursive flex over Member tree -> Pane renders tab bar + active Item; docks host panels |
| Terminal | `terminal` (terminal.rs, alacritty/, mappings/), `terminal_view` (terminal_view.rs, terminal_element.rs, terminal_panel.rs) | `Terminal` (PTY + alacritty grid), `TerminalView`, `TerminalElement`, `TerminalPanel` | PTY read -> vte::Processor -> grid mutation; TerminalElement iterates visible cells, batches text runs, paints bg/text/cursor/selection |
| File tree | `project_panel` (project_panel.rs), `worktree` (worktree.rs), `project` (project.rs), `file_icons`, `git` | `ProjectPanel`, `Worktree` (Entry tree + snapshot + fs watcher + git status), `Entry`, `ProjectPath` | fs watcher -> Entry snapshot -> flatten expanded set into `visible_entries` -> UniformList row (indent, icon, name, git badge, diagnostic) |

### Shared foundations they all need
- Essential: `gpui` (Entity/Context/Window/Element/Render, layout, input, executor), `text`+`rope` (Rope, Anchor, Selection, Point), `sum_tree` (aggregating B-tree for O(log n) range queries), `multi_buffer`, `language` (tree-sitter + HighlightMap), `settings`+`theme`.
- Can shim/defer: `collab`/`rpc` (remote), `lsp` (completions/diagnostics — defer), `language_model`/agent (LLM), `dap` (debug), `vim`.

### Deepest risks when porting
1. `DisplayMap` transform stack (fold -> tab -> wrap -> block -> inlay) — stateful, delta-updated, subtle.
2. `EditorElement` 8-layer paint ordering.
3. `PaneGroup` recursive flex layout over the Member tree.
Note: we do NOT clone gpui; we keep wgpu + glyphon and build a thin retained-UI layer.

## Full crate catalog (246 crates)

One line per folder — what it does and why it exists.

### a - cl
| crate | purpose |
|---|---|
| acp_thread | Agent-client-protocol thread/session management: connections, messages, user interactions. |
| acp_tools | UI for ACP debug logs/messages inside the workspace. |
| action_log | Tracks undo info for rejected agent edits (per-buffer edits, file-create metadata). |
| activity_indicator | Status-bar indicator for LSP health, background fs ops, extension updates. |
| agent | Core agent orchestration: sessions, thread execution, tool permissions, model selection, LLM integration. |
| agent_servers | Manages agent server backends (ACP + custom), connection/protocol delegation. |
| agent_settings | Config for agent behavior, UI layout, model params. |
| agent_skills | YAML skill discovery/management (project + global scope). |
| agent_ui | Agent UI: panels, inline assistants, thread views, completion providers. |
| ai_onboarding | Onboarding for AI setup (API keys, plan info). |
| anthropic | Anthropic Claude API client (streaming, batches, extended thinking). |
| askpass | SSH password prompt handler (Unix sockets / native Windows dialogs). |
| assets | Embedded asset loader (fonts, icons, themes, sounds, prompts). |
| audio | Audio playback/record via Rodio; call/agent sound effects. |
| auto_update | App update check/download with release notes and staged install. |
| auto_update_helper | Windows update-apply helper binary with progress UI. |
| auto_update_ui | In-window update notification/release-notes UI. |
| aws_http_client | Adapts Zed HttpClient to AWS Smithy runtime (Bedrock). |
| bedrock | AWS Bedrock API client (streaming, tool use, extended thinking). |
| benchmarks | Criterion benchmarks for editor/display-map/tool perf. |
| breadcrumbs | Toolbar breadcrumb of active file path. |
| buffer_diff | Diff/patch engine (word/line level, stage/unstage). |
| call | VoIP + screen sharing via LiveKit. |
| call_hierarchy | LSP call hierarchy browser (incoming/outgoing) in a picker. |
| channel | Collaborative channel storage/sync over client RPC. |
| cli | `zed` CLI to open files/projects with behavior flags. |
| client | Core network client: RPC, auth, telemetry, cloud API, proxy. |
| clock | Abstract/distributed clock for CRDT ops + testing. |
| cloud_api_client | HTTP client for Zed cloud backend (auth, websockets). |
| cloud_api_types | Types/schemas for Zed cloud API. |
| cloud_llm_client | Types/header constants for Zed cloud LLM service. |

### co - gr
| crate | purpose |
|---|---|
| codestral | Codestral model integration for completion. |
| collab | Real-time collaboration server + RPC protocols. |
| collab_ui | UI for collaboration, chat, notifications. |
| collections | Specialized collections (VecMap, IndexMap integration). |
| command_palette | Command palette UI + action search. |
| command_palette_hooks | Dynamic command-palette action registration. |
| component | Component system (layout/rendering infra). |
| component_preview | Component inspection/preview UI. |
| context_server | MCP-style context provider for AI models. |
| copilot | GitHub Copilot integration for suggestions/edit prediction. |
| copilot_chat | Copilot chat UI + conversation. |
| copilot_ui | Copilot auth/interaction UI. |
| dap | Debug Adapter Protocol implementation/client. |
| dap_adapters | Per-language DAP adapters (Python, JS, Go, GDB, CodeLLDB). |
| db | Persistent key-value DB abstraction. |
| debug_adapter_extension | Extension API for debug adapters. |
| debugger_tools | Debugging tools/utilities. |
| debugger_ui | Debugger panels/modals/session UI. |
| deepseek | DeepSeek model integration. |
| dev_container | Dev-container config parser + runtime. |
| diagnostics | Diagnostic rendering for LSP errors/warnings. |
| docs_preprocessor | Preprocessor for Zed docs generation. |
| edit_prediction | Edit prediction / fill-in-middle integration. |
| edit_prediction_cli | CLI for eval/testing edit prediction. |
| edit_prediction_context | Context extraction for edit prediction. |
| edit_prediction_metrics | Edit-prediction quality metrics. |
| edit_prediction_types | Shared edit-prediction types. |
| edit_prediction_ui | Edit-prediction settings/onboarding UI. |
| editor | Core text editor: highlighting, completions, language features. |
| editor_benchmarks | Editor-operation benchmarks. |
| encoding_selector | UI to pick file character encoding. |
| env_var | Env-var expansion/substitution. |
| etw_tracing | Windows ETW perf diagnostics. |
| eval_cli | Headless CLI for agent eval/benchmark. |
| eval_utils | Eval task utilities. |
| explorer_command_injector | Inject custom commands into UI explorers. |
| extension | Extension API + runtime loader. |
| extension_api | Public extension API defs. |
| extension_cli | Extension dev/publish CLI. |
| extension_host | WASM extension runtime host. |
| extensions_ui | Extension marketplace/management UI. |
| feature_flags | Feature-flag system. |
| feature_flags_macros | Compile-time feature-flag macros. |
| feedback | Feedback submission + telemetry. |
| file_finder | File search/navigation UI (Cmd+P). |
| file_icons | Extension -> icon mapping. |
| fs | Filesystem abstraction with watching/ops. |
| fs_benchmarks | Filesystem op benchmarks. |
| fuzzy | Fuzzy string matching/scoring. |
| fuzzy_nucleo | Fuzzy search via nucleo. |
| git | Git integration/repo operations. |
| git_hosting_providers | GitHub/GitLab/Gitea API integrations. |
| git_ui | Git UI panels/controls. |
| git_ui_core | Core Git UI model/logic. |
| go_to_line | Go-to-line dialog. |
| google_ai | Google Gemini integration. |
| gpui | GPU-accelerated UI framework (layout, input, rendering). |
| gpui_apple / gpui_macos | macOS platform impl / windowing+events. |
| gpui_linux | Linux platform impl. |
| gpui_windows | Windows platform impl. |
| gpui_web | Web (Wasm/Canvas) platform impl. |
| gpui_wgpu | wgpu rendering backend for gpui. |
| gpui_platform | Platform abstraction (windowing, input, fonts). |
| gpui_macros | Element/Render/RenderOnce derives. |
| gpui_shared_string | Arc-backed interned string. |
| gpui_tokio | Tokio runtime integration. |
| gpui_util | gpui utilities (futures, collections, helpers). |
| grammars | Tree-sitter grammar loading + highlighting. |

### h - o
| crate | purpose |
|---|---|
| html_to_markdown | HTML -> Markdown conversion. |
| http_client | HTTP client for Zed/gpui. |
| http_client_tls | TLS config/cert verification (rustls). |
| http_proxy | Local HTTP/HTTPS proxy (chaining, SSRF protection). |
| icons | Icon-identifier enum. |
| image_viewer | Image-viewing editor item. |
| input_latency_ui | Input-to-frame latency reports. |
| inspector_ui | GPUI div/component hierarchy inspector. |
| install_cli | Registers `zed://` scheme, installs CLI. |
| journal | Daily journal entries with timestamps. |
| json_schema_store | JSON schemas for config files. |
| keymap_editor | Keybinding editor UI/completion. |
| language | Core language support (syntax trees, highlighting, detection). |
| language_core | Highlighting/symbols/syntax-layer data structures. |
| language_detection | Detect language from content. |
| language_extension | LSP integration for extension languages. |
| language_model | Image encode/resize for LLM vision. |
| language_model_core | Chat completion protocols, rate limiting, LM abstraction. |
| language_models | LLM provider impls (Anthropic/OpenAI/Ollama...). |
| language_models_cloud | Zed cloud LM integration. |
| language_onboarding | Language-server setup banners/guidance. |
| language_selector | Status-bar active-language switcher. |
| language_tools | Tree-sitter syntax-tree inspector panel. |
| languages | Language defs + LSP adapter configs. |
| line_ending_selector | CRLF/LF status-bar selector. |
| livekit_api | LiveKit server API SDK. |
| livekit_client | gpui LiveKit audio/video client. |
| llama_cpp | Local llama.cpp provider. |
| lmstudio | LM Studio provider. |
| lsp | Language Server Protocol impl (message read/parse/dispatch). |
| lsp_command_selector | Picker for LSP code actions/commands. |
| lsp_locations | Navigate LSP result locations with picker fallback. |
| markdown | Markdown parse/render (mermaid, zoom). |
| markdown_preview | Markdown preview settings/UI. |
| media | macOS Core Media bindings (codecs, sample buffers). |
| menu | Menu navigation actions (select/confirm/cancel). |
| mermaid_render | Mermaid SVG post-processing. |
| migrator | DB schema migration/versioning. |
| miniprofiler_ui | Profiler visualization UI. |
| mistral | Mistral AI provider. |
| multi_buffer | Multi-file editing over shared undo/redo. |
| net | Network utilities / async TCP. |
| node_runtime | Node.js runtime mgmt (proxy, CA). |
| notifications | Notification center/display. |
| oauth_callback_server | Loopback OAuth server for sign-in. |
| ollama | Ollama provider. |
| onboarding | New-user setup UI/tutorials. |
| open_ai | OpenAI-compatible provider. |
| open_path_prompt | Open-path file dialog. |
| open_router | OpenRouter aggregator provider. |
| openai_subscribed | Codex CLI wrapper for subscription access. |
| opencode | Opencode model provider. |
| outline | Language-aware document outline. |
| outline_panel | Outline panel UI/settings. |

### p - z
| crate | purpose |
|---|---|
| path / paths | Normalized UTF-8 path types / platform data dirs. |
| picker | Modal fuzzy-list picker component. |
| picker_preview | Editor-backed preview for the picker. |
| platform_title_bar / title_bar | Native window title bar with tabs/menus. |
| prettier | Prettier formatter via Node. |
| project | Project mgmt: file indexing, LSP coordination, worktrees. |
| project_benchmarks | Project benchmarks. |
| project_panel | File-tree UI panel + ops. |
| project_symbols | Project symbol picker. |
| prompt_store | Custom prompt DB + rules->skills migration. |
| proto | Protobuf defs/serialization for networking. |
| proxy_handshake | Sans-IO HTTP CONNECT / SOCKS. |
| recent_projects | Recent-projects modal/sidebar. |
| refineable | Refinement-type macro for partial struct init. |
| release_channel | Version/channel (stable/dev/nightly) mgmt. |
| remote / remote_connection / remote_server | Remote editing client / connect modal / daemon. |
| repl | Interactive notebook REPL. |
| reqwest_client | reqwest-based HTTP client. |
| rope | Text rope for large-string manipulation. |
| rpc | RPC protocol/connection mgmt. |
| sandbox | Process sandboxing (Seatbelt/bwrap/WSL). |
| scheduler | Async task scheduling. |
| schema_generator | JSON-schema codegen. |
| search | Buffer/project search UI + actions. |
| session | Window-persistence session tracking. |
| settings / settings_content / settings_json / settings_macros / settings_ui | Settings system, generated content, JSON parse, macros, UI. |
| settings_profile_selector | Settings-profile selector UI. |
| shell_command_parser | Shell command template parser. |
| sidebar | Sidebar panel (project/terminal/recent). |
| snippet / snippet_provider / snippets_ui | Snippet data/parse / LSP provider / picker. |
| sqlez / sqlez_macros | Async SQLite wrapper + macros. |
| streaming_diff | Streaming diff algorithm. |
| sum_tree | Aggregating B-tree for concurrent apps. |
| svg_preview | SVG preview with live editor sync. |
| syntax_theme | Syntax highlight theme defs/application. |
| system_specs | System/hardware info. |
| tab_switcher | Quick tab-switch modal. |
| tabular_data_preview | Spreadsheet-like data preview. |
| task / tasks_ui | Task defs/templates/exec / task UI. |
| telemetry / telemetry_events | Telemetry collection / typed events. |
| terminal | Terminal emulator (alacritty backend). |
| terminal_view | Embedded terminal pane UI. |
| text | OT/CRDT for collaborative text editing. |
| theme / theme_extension / theme_importer / theme_selector / theme_settings | Theme system / extension API / import / selector / settings. |
| time_format | Timestamp formatting. |
| toolchain_selector | Build-toolchain selector UI. |
| ui / ui_input / ui_macros / ui_prompt | Core UI primitives / inputs / macros / prompts. |
| util / util_macros | Utilities / macros. |
| vim / vim_mode_setting | Vim modal editing / mode setting. |
| watch | SPMC latest-value channels. |
| web_search / web_search_providers | Web search / providers. |
| which_key | Which-key command hints. |
| windows_resources | Windows resources/manifests. |
| workspace | Workspace: panes, items, docks, collaboration. |
| worktree / worktree_benchmarks | Filesystem tree + monitoring / benchmarks. |
| x_ai / x_ai_subscribed | X.AI provider / subscription. |
| zed | Main app entry + init. |
| zed_actions | Standard editor/UI actions. |
| zed_credentials_provider | Credential storage/retrieval. |
| zed_env_vars | Env-var management. |
| zeta_prompt | Zeta multi-region prompt format. |
| zlog / zlog_settings | Logging system / settings. |
| ztracing / ztracing_macro | Tracing/profiling instrumentation. |

## Swift -> Rust migration roadmap

Direction: stop adding Swift; build the app in Rust (winit + wgpu + glyphon), cross-platform.
`pomelo-editor-kit` becomes the app core. Phased so each phase is runnable.

- Phase 0 (done): GPU editor kit — rope buffer, tree-sitter highlight (One Dark), scroll/cull,
  caret+blink, selection, click/drag, embedded via CAMetalLayer FFI.
- Phase 1 — Rust window shell: promote `main.rs` (winit) into a real app: window + a retained-UI
  layer (rects + text via the existing renderer) with input routing. Replaces the AppKit host.
- Phase 2 — Pane/Tabs in Rust: `Member = Pane|Axis` tree, flex splits, tab bar, drag between panes
  (port `workspace/pane_group.rs`). Editor becomes one Item type.
- Phase 3 — File tree in Rust: `worktree` (fs walk + `notify` watcher + gitignore via `ignore`) +
  `project_panel` (flatten visible entries, icons, git badge, rename/create/delete).
- Phase 4 — Terminal in Rust: `alacritty_terminal` grid + PTY (`portable-pty`) + a grid element,
  hosted as a bottom-dock pane.
- Phase 5 — Editor depth: undo/redo + Anchors, multi-cursor + shift-select, soft-wrap + tab-expand,
  then LSP (defer until the shell is solid).
- Phase 6 — Port remaining Swift features (git panel, PR management, services) to Rust panels; drop
  the Swift target.

FFI note: until Phase 1 lands, the kit stays embedded in Swift via the C ABI (`ffi.rs`). Once the
Rust shell hosts the editor natively, the FFI is only needed for any residual Swift screens.
