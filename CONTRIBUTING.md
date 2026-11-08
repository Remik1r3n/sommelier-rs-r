# 如何贡献

`sommelier-rs-r` 是独立的 Rust 实现，以主线 C++ `sommelier` 的行为为兼容基准，
**不是 Google 官方项目，也不属于 ChromiumOS 组件**。请把改动提交到本仓库，
不要发往上游 `sommelier-rs` 或 ChromiumOS。

## 本地构建与测试

系统依赖见 [`README.md`](README.md)「构建」。在 Linux / Crostini 上：

```bash
cargo fmt --all -- --check
cargo check --workspace --tests
cargo test  --workspace
cargo build --release --workspace --locked
```

Windows 开发机没有 Linux linker，只能做交叉类型检查：

```bash
cargo check --workspace --tests --target x86_64-unknown-linux-gnu
```

真机行为（`/dev/wl0`、Xwayland、systemd `Type=notify`）无法在开发机验证，
涉及这些路径的改动请在 Crostini 上按
[`docs/CROSTINI_INSTALL.md`](docs/CROSTINI_INSTALL.md) 实测。

## 构建 Debian 包

```bash
bash packaging/build-deb.sh          # 产物在 dist/
```

脚本必须在 Debian 13 上运行：它用 `dpkg-shlibdeps` 依据本机 freshly built
binary 解析运行时依赖。CI 里的 release workflow 调用同一个脚本，产出 `.deb`。

## 提交改动

- 源码与文档同步更新：主线行为对照写
  [`docs/MAINLINE_PARITY.md`](docs/MAINLINE_PARITY.md)，功能状态写
  [`docs/FEATURE_MATRIX.md`](docs/FEATURE_MATRIX.md)，结构变化写
  [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)。
- `config.rs` 中标注为 *mainline compatibility flags* 的参数即使当前不被读取，
  也属于主线 CLI 契约；不要当作死代码删除。三套 transport
  （`Unix` / `VirtWayland` / `VirtGpu`）同理，都是运行时支持的能力。
- 新增协议 XML 放进 `third_party/protocols/` 即可，`build.rs` 会自动枚举。
- 提交前跑完上述检查，并保证 `.github/workflows/ci-debian.yaml` 通过。
- 提交信息写清「改了什么、为什么」；涉及主线语义时附主线源码位置
  （例如 `sommelier.cc:4253`）。

## 许可

Apache-2.0，见 [LICENSE](LICENSE)。新增源码文件请沿用现有版权头。
