# vendor/ — GPUI への GPU backdrop-filter 追加 (Liquid Glass の下地)

`Cargo.toml` の `[patch.crates-io]` が、crates.io の `gpui-pre` / `gpui-pre-windows` **0.3.11**
(macOS 描画は `gpui-pre-apple` / `gpui-pre-macos` **0.3.12**) の代わりに
このディレクトリを使う。どちらも Apache-2.0 (各 `LICENSE-APACHE` を参照)。元は Zed の GPUI。
改変箇所には `DEVHUB` のコメントを付けている。

## 原本との差分 (crates.io 0.3.11 比。apple/macos は 0.3.12 比)

| ファイル | 内容 |
| --- | --- |
| `gpui-pre/src/elements/div.rs`, `style.rs`, `window.rs`, `scene.rs` | backdrop 要素の API と `Scene::backdrops` の追加 |
| `gpui-pre-windows/src/backdrop.rs` (新規) | ダウンサンプル → 分離ブラー → 屈折・彩度・明るさ・リムライトの合成 |
| `gpui-pre-windows/src/gpu_timer.rs` (新規) | `DEVHUB_GPUI_GPUTIME=1` のときだけ動く GPU タイムスタンプ計測 |
| `gpui-pre-windows/src/directx_renderer.rs`, `shaders.hlsl`, `build.rs` | backdrop 用パスとシェーダーの組み込み |
| `gpui-pre-apple/src/metal_renderer.rs`, `shaders.metal` | `PolychromeSprites` の `nearest` を最近傍サンプラのパイプライン (`polychrome_sprite_nearest_fragment`) で受ける。`PrimitiveBatch::Backdrops` は読み捨て (macOS は GPU backdrop 非対応、非ガラス経路で描画) |
| `gpui-pre-macos/src/window.rs` | Rust 1.95 で未安定の `Option::map_or_default` を match に展開 (3 箇所) |

`Cargo.toml.orig` は crates.io 配布物のもの。行末が原本と違うため、単純な diff は行数が膨らむ。
差分を取るなら `diff --strip-trailing-cr` を使う。

## 更新手順

1. `Cargo.toml` の gpui-pre のバージョンを上げる前に、新版の原本を `~/.cargo/registry/src/` から取り出す。
2. 上の表のファイルへ `DEVHUB` 付きの変更を移植し、`cargo test --locked` と `cargo check --locked --features perf` を通す。
3. 背景 (`/tech` と GPUI の glass) が崩れていないことを実機で見る。

## 履歴

- 2026-11-11: `liquid_glass.hlsl` に面ごとの「右辺・下辺のリム」を消す手段を足した。
  GPU の Liquid Glass 経路は面自身の縁 (`Material::edge`) を描かないため、ナビの右辺に出る
  1px の明るい線は素材の 값을変えても消せなかった (`Backdrop::rim[1]` を流用して強さを渡す:
  0 = 既定 / 正 = その倍率 / 負 = 消灯)。既定の値は 1.0 のままなので、参照実装との一致は
  変わらない (tools/glass-fidelity.ps1 の比較対象も同じ)。

- 2026-11-10: `shaders.hlsl` の外側の影を要素の輪郭で切り抜くよう修正。
  半透明のガラスが自身の影を背景として取り込み、白いカードとナビが灰色になる不具合を解消。
  内側の影は従来どおり要素の内側に描く。

- 2026-11-10: `1234igor/gpui-liquid-glass` の固定リビジョン `c002c4d3758f934fdfaf173e8214a24eb65a05cd` から
  Regular / Clear の素材式を `gpui-pre-windows/src/liquid_glass.hlsl` へ移植。Windows の領域単位の
  snapshot、縮小 Gaussian と組み合わせ、dark tint と light Clear のコントラストを調整。
  `Backdrop::glass` で素材と操作エネルギーを渡す。同一層の隣り合う同一素材を統合する。
  移植元の MIT ライセンスは [gpui-liquid-glass-LICENSE-MIT](../licenses/gpui-liquid-glass-LICENSE-MIT)、
  GPUI の Apache-2.0 は各 vendor ディレクトリ内の `LICENSE-APACHE` を参照。どちらも Windows 配布物に同梱する。

- 2026-10-13: gpui-kit 0.7.3 に合わせて 0.3.7 → 0.3.9。上の表の差分はそのまま当たった (window.rs は 182 行ずれただけ)。
  描画 (directx_renderer.rs / shaders.hlsl / style.rs) は上流で変化なし。旧版 (0.1.15) と 5 画面の撮影が画素単位で一致。
- 2026-11-05: gpui-kit 0.9.0 に合わせて 0.3.9 → 0.3.11。
  div.rs / window.rs / scene.rs / directx_renderer.rs / shaders.hlsl は上流が変わっていたため三方マージした
  (div.rs の smooth_wheel 宣言 2 行と scene.rs の 1 ハンクは手当て)。shaders.hlsl と directx_renderer.rs は
  そのまま当たった。build.rs は上流で変更なし。
