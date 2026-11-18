//! devhub-obs — OBS ソースプラグイン。
//!
//! `devhub-obs.dll` を `C:\ProgramData\obs-studio\plugins\devhub_obs\bin\64bit\`
//! に置くと、ソース追加一覧に「Device HUB Live / キーボード / マウス」が出る。
//! (OBS 30.2+ は %APPDATA%\plugins をスキャンしない。)
//! obs.dll は実行時に GetProcAddress で解決するので import lib は要らない。

pub mod config;
#[cfg(windows)]
mod ffi;
pub mod live;
pub mod rawinput;
pub mod render;

// DEVHUB: ここから下は OBS プラグイン本体 (Windows 専用)。macOS は
// config / live / rawinput / render (アプリ内プレビュー・examples 用) だけを提供する。

#[cfg(windows)]
use core::ffi::c_void;
#[cfg(windows)]
use std::collections::HashSet;
#[cfg(windows)]
use std::ffi::CString;
#[cfg(windows)]
use std::sync::atomic::AtomicPtr;
#[cfg(windows)]
use std::sync::Mutex;

#[cfg(windows)]
static MODULE: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());

/// 起動診断ログ (%TEMP%\devhub_obs.log)。ホスト側の obs log には出せないので
/// ファイルに直接書く — 問題切り分け用、常時 ON。
#[cfg(windows)]
fn logline(s: &str) {
    use std::io::Write;
    let p = format!(r"{}\devhub_obs.log", std::env::temp_dir().display());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
    {
        let _ = writeln!(f, "{:?} {s}", std::time::Instant::now());
    }
}

#[cfg(windows)]
fn cs(s: &str) -> CString {
    CString::new(s).unwrap_or_else(|_| CString::new("?").unwrap())
}

#[cfg(windows)]
struct Src {
    mode: render::Mode,
    font: Option<fontdue::Font>,
    pix: Option<render::Pix>,
    tex: *mut c_void,
    last_seq: u64,
    /// 最後に反映したアプリ設定 (obs_overlay.json) の世代。
    last_cfg: u64,
    dirty: bool,
    /// 初回描画の診断用。
    first_render: Option<bool>,
    /// プロパティ設定 (update() が UI スレッドから書く)。
    vis: Mutex<render::Vis>,
    /// 補間のターゲットとなる最新スナップショット。
    live: live::Live,
    /// 表示中の補間値: キーごとの route (mm、samples 順) / ボタンごとの depth (0..1)。
    /// データ到着 (~15-25Hz) より描画 (~60fps) のほうが速いので、
    /// 値をそのまま描くと紙芝居になる — 指数補間で滑らかにする。
    sm_route: Vec<f32>,
    sm_depth: Vec<f32>,
    last_tick: std::time::Instant,
}

#[cfg(windows)]
impl Src {
    fn new(mode: render::Mode) -> Self {
        Self {
            mode,
            font: render::font(),
            pix: None,
            tex: core::ptr::null_mut(),
            last_seq: u64::MAX,
            last_cfg: 0,
            dirty: false,
            first_render: None,
            vis: Mutex::new(render::Vis::default()),
            live: live::Live::default(),
            sm_route: Vec::new(),
            sm_depth: Vec::new(),
            last_tick: std::time::Instant::now(),
        }
    }

    /// 補間値をターゲットへ近づける。まだ動いている (再描画が要る) なら true。
    fn advance(&mut self, dt: f32) -> bool {
        const TAU: f32 = 0.045; // s — 60fps なら ~3 フレームで 9 割追従
        let k = 1.0 - (-dt / TAU).exp();
        let mut moving = false;
        match &self.live.kb {
            Some(kb) => {
                if self.sm_route.len() != kb.samples.len() {
                    // 構成変更時はその場で追いつく (初回のスイープイン防止)。
                    self.sm_route = kb.samples.iter().map(|s| s.route).collect();
                } else {
                    for (sm, t) in self
                        .sm_route
                        .iter_mut()
                        .zip(kb.samples.iter().map(|s| s.route))
                    {
                        let d = t - *sm;
                        if d.abs() > 0.004 {
                            *sm += d * k;
                            moving = true;
                        } else {
                            *sm = t;
                        }
                    }
                }
            }
            None => self.sm_route.clear(),
        }
        match &self.live.mouse {
            Some(m) => {
                if self.sm_depth.len() != m.depths.len() {
                    self.sm_depth = m.depths.iter().map(|b| b.depth).collect();
                } else {
                    for (sm, t) in self
                        .sm_depth
                        .iter_mut()
                        .zip(m.depths.iter().map(|b| b.depth))
                    {
                        let d = t - *sm;
                        if d.abs() > 0.003 {
                            *sm += d * k;
                            moving = true;
                        } else {
                            *sm = t;
                        }
                    }
                }
            }
            None => self.sm_depth.clear(),
        }
        moving
    }

    /// 補間値で route/depth を差し替えた描画用スナップショット。
    fn smoothed_live(&self) -> live::Live {
        let mut l = self.live.clone();
        if let Some(kb) = &mut l.kb {
            for (smp, sm) in kb.samples.iter_mut().zip(&self.sm_route) {
                smp.route = *sm;
            }
        }
        if let Some(m) = &mut l.mouse {
            for (b, sm) in m.depths.iter_mut().zip(&self.sm_depth) {
                b.depth = *sm;
            }
        }
        l
    }
}

// ---------------- ソースコールバック ----------------

#[cfg(windows)]
unsafe extern "C" fn name_both(_d: *mut c_void) -> *const u8 {
    lt(c"devhub.src.both", c"Device HUB Live")
}
#[cfg(windows)]
unsafe extern "C" fn name_kb(_d: *mut c_void) -> *const u8 {
    lt(c"devhub.src.kb", c"Device HUB Keyboard")
}
#[cfg(windows)]
unsafe extern "C" fn name_mouse(_d: *mut c_void) -> *const u8 {
    lt(c"devhub.src.mouse", c"Device HUB Mouse")
}


#[cfg(windows)]
unsafe extern "C" fn create_both(_s: *mut c_void, _src: *mut c_void) -> *mut c_void {
    logline("create (both)");
    Box::into_raw(Box::new(Src::new(render::Mode::Both))) as *mut c_void
}
#[cfg(windows)]
unsafe extern "C" fn create_kb(_s: *mut c_void, _src: *mut c_void) -> *mut c_void {
    logline("create (kb)");
    Box::into_raw(Box::new(Src::new(render::Mode::Kb))) as *mut c_void
}
#[cfg(windows)]
unsafe extern "C" fn create_mouse(_s: *mut c_void, _src: *mut c_void) -> *mut c_void {
    logline("create (mouse)");
    Box::into_raw(Box::new(Src::new(render::Mode::Mouse))) as *mut c_void
}

#[cfg(windows)]
unsafe extern "C" fn destroy(d: *mut c_void) {
    if !d.is_null() {
        drop(Box::from_raw(d as *mut Src));
    }
}

/// サイズは表示キー・テーマ・倍率で変わるので、最後に焼いたビットマップに合わせる。
#[cfg(windows)]
unsafe extern "C" fn get_width(d: *mut c_void) -> u32 {
    let s = &*(d as *mut Src);
    match &s.pix {
        Some(p) => p.w,
        None => render::size(&render_default_live(), s.mode, &s.cur_vis()).0,
    }
}
#[cfg(windows)]
unsafe extern "C" fn get_height(d: *mut c_void) -> u32 {
    let s = &*(d as *mut Src);
    match &s.pix {
        Some(p) => p.h,
        None => render::size(&render_default_live(), s.mode, &s.cur_vis()).1,
    }
}

#[cfg(windows)]
fn render_default_live() -> live::Live {
    live::Live::default()
}

#[cfg(windows)]
impl Src {
    /// OBS プロパティ + アプリ設定を合成した描画設定。
    /// `obs_all` (「すべての表示設定を OBS で行う」) のソースはアプリ設定を見ない。
    fn merge_vis(&self, cfg: &config::OverlayCfg) -> render::Vis {
        let v = self
            .vis
            .lock()
            .map(|v| v.clone())
            .unwrap_or_default();
        if v.obs_all {
            v
        } else {
            v.with_cfg(cfg)
        }
    }

    fn cur_vis(&self) -> render::Vis {
        let (cfg, _) = config::current();
        self.merge_vis(&cfg)
    }
}

/// OBS 設定文字列を Rust String に (UTF-8、先頭 max バイトまで)。
#[cfg(windows)]
unsafe fn cstr_text(p: *const u8, max: usize) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut buf = Vec::new();
    let mut e = p;
    while *e != 0 && buf.len() < max {
        buf.push(*e);
        e = e.add(1);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// OBS のプラグインロケール (`data/locale/<lang>.ini`) で訳文を引く。
/// 未ロード/キー無し/古い OBS なら fallback (英語) を返す。
/// 返るポインタは OBS のルックアップ表・static・または引数の CStr — いずれも
/// この式の評価中は生きており、OBS は desc をコピーして持つので安全。
/// (get_name/description に渡す fallback は 'static の CStr に限る)
#[cfg(windows)]
unsafe fn lt(key: &std::ffi::CStr, fallback: &std::ffi::CStr) -> *const u8 {
    let m = MODULE.load(std::sync::atomic::Ordering::Relaxed);
    if let (Some(api), false) = (ffi::resolve(), m.is_null()) {
        if let Some(loc) = api.module_locale {
            let p = loc(m, key.as_ptr() as *const u8);
            if !p.is_null() {
                return p;
            }
        }
    }
    fallback.as_ptr() as *const u8
}

/// OBS の色データ (int、下位から R,G,B,A の 8bit) を pack/unpack。
#[cfg(windows)]
fn pack_rgb(r: i64, g: i64, b: i64) -> i64 {
    r | (g << 8) | (b << 16) | (255i64 << 24)
}
#[cfg(windows)]
fn unpack_rgb(v: i64) -> [u8; 4] {
    [
        (v & 0xff) as u8,
        ((v >> 8) & 0xff) as u8,
        ((v >> 16) & 0xff) as u8,
        255,
    ]
}

/// グラフィックススレッド — テクスチャを捨てる (create 側で遅延生成)。
#[cfg(windows)]
unsafe extern "C" fn deactivate(d: *mut c_void) {
    let s = &mut *(d as *mut Src);
    if !s.tex.is_null() {
        if let Some(api) = ffi::resolve() {
            (api.tex_destroy)(s.tex);
        }
        s.tex = core::ptr::null_mut();
    }
}

// ---- プロパティ (設定 UI) ----

/// 既定値: タイトル/ラベル ON、アクセント=アプリ設定。`obs_cfg` は OFF — 既存シーンの
/// 見た目を変えない。詳細設定は OverlayCfg の既定と同じ (ON にしたときそのまま使える)。
#[cfg(windows)]
unsafe extern "C" fn get_defaults(settings: *mut c_void) {
    let Some(api) = ffi::resolve() else { return };
    let dbool = |n: &std::ffi::CStr, v: bool| {
        (api.data_set_default_bool)(settings, n.as_ptr() as *const u8, v)
    };
    let dstr = |n: &std::ffi::CStr, v: &std::ffi::CStr| {
        (api.data_set_default_string)(settings, n.as_ptr() as *const u8, v.as_ptr() as *const u8)
    };
    dbool(c"title", true);
    dbool(c"labels", true);
    dstr(c"accent", c"app");
    dbool(c"obs_cfg", false);
    // ---- 「すべての表示設定を OBS で行う」の既定 (OverlayCfg::default と同じ) ----
    dstr(c"theme", c"glass");
    dbool(c"numbers", false);
    dstr(c"mstyle", c"both");
    dstr(c"arrange", c"side");
    dstr(c"fill", c"bottom");
    dbool(c"num_pct", false);
    dbool(c"badges", true);
    dbool(c"tline", true);
    dbool(c"shadow", true);
    (api.data_set_default_double)(settings, c"oscale".as_ptr() as *const u8, 1.0);
    (api.data_set_default_double)(settings, c"ksize".as_ptr() as *const u8, 1.0);
    (api.data_set_default_double)(settings, c"radius".as_ptr() as *const u8, 1.0);
    (api.data_set_default_double)(settings, c"opad".as_ptr() as *const u8, 1.0);
    (api.data_set_default_int)(settings, c"palpha".as_ptr() as *const u8, 100);
    (api.data_set_default_int)(settings, c"glow".as_ptr() as *const u8, 100);
    dstr(c"ttext", c"");
    dbool(c"pcust", false);
    (api.data_set_default_int)(settings, c"acolor".as_ptr() as *const u8, pack_rgb(60, 190, 255));
    (api.data_set_default_int)(settings, c"pcolor".as_ptr() as *const u8, pack_rgb(255, 150, 60));
}

/// 設定読み込み。`sk{i}`/`sb{i}` が true のインデックスを表示集合に入れる
/// (全て false = すべて表示。1 つでも true があればそれだけを表示)。
#[cfg(windows)]
unsafe extern "C" fn update(d: *mut c_void, settings: *mut c_void) {
    let Some(api) = ffi::resolve() else { return };
    let s = &mut *(d as *mut Src);
    let getb = |n: &std::ffi::CStr| (api.data_get_bool)(settings, n.as_ptr() as *const u8);
    let mut vis = render::Vis {
        title: getb(c"title"),
        labels: getb(c"labels"),
        obs_all: getb(c"obs_cfg"),
        ..render::Vis::default()
    };
    // 「すべての表示設定を OBS で行う」が ON のときは詳細項目を全部読む
    // (OFF なら値は捨てて、cur_vis 側でアプリ設定が使われる)。
    if vis.obs_all {
        use config::{Arrange, FillMode, MouseStyle, Theme};
        let gets = |n: &std::ffi::CStr, m| cstr_text((api.data_get_string)(settings, n.as_ptr() as *const u8), m);
        vis.theme = match gets(c"theme", 8).as_str() {
            "light" => Theme::Light,
            "clear" => Theme::Clear,
            "neon" => Theme::Neon,
            _ => Theme::Glass,
        };
        vis.numbers = getb(c"numbers");
        vis.mouse_style = match gets(c"mstyle", 8).as_str() {
            "shape" => MouseStyle::Shape,
            "bars" => MouseStyle::Bars,
            _ => MouseStyle::Both,
        };
        vis.arrange = match gets(c"arrange", 8).as_str() {
            "stack" => Arrange::Stack,
            "swap" => Arrange::Swap,
            _ => Arrange::Side,
        };
        vis.fill_mode = match gets(c"fill", 8).as_str() {
            "top" => FillMode::Top,
            "fade" => FillMode::Fade,
            _ => FillMode::Bottom,
        };
        vis.num_pct = getb(c"num_pct");
        vis.badges = getb(c"badges");
        vis.trigger_line = getb(c"tline");
        vis.shadow = getb(c"shadow");
        vis.title_text = gets(c"ttext", 64);
        if getb(c"pcust") {
            vis.pressed = Some(unpack_rgb(
                (api.data_get_int)(settings, c"pcolor".as_ptr() as *const u8),
            ));
        }
        vis.scale = (api.data_get_double)(settings, c"oscale".as_ptr() as *const u8).clamp(0.75, 3.0) as f32;
        vis.key_size = (api.data_get_double)(settings, c"ksize".as_ptr() as *const u8).clamp(0.7, 1.4) as f32;
        vis.radius = (api.data_get_double)(settings, c"radius".as_ptr() as *const u8).clamp(0., 2.) as f32;
        vis.pad = (api.data_get_double)(settings, c"opad".as_ptr() as *const u8).clamp(0.5, 2.) as f32;
        vis.panel_alpha = ((api.data_get_int)(settings, c"palpha".as_ptr() as *const u8).clamp(0, 100) as f32) / 100.;
        vis.glow = ((api.data_get_int)(settings, c"glow".as_ptr() as *const u8).clamp(0, 200) as f32) / 100.;
    }
    let mut show_kb = HashSet::new();
    for i in 0..160 {
        let n = cs(&format!("sk{i}"));
        if (api.data_get_bool)(settings, n.as_ptr() as *const u8) {
            show_kb.insert(i);
        }
    }
    let mut show_btn = HashSet::new();
    for i in 0..24 {
        let n = cs(&format!("sb{i}"));
        if (api.data_get_bool)(settings, n.as_ptr() as *const u8) {
            show_btn.insert(i);
        }
    }
    vis.show_kb = show_kb;
    vis.show_btn = show_btn;
    let acc = cstr_text(
        (api.data_get_string)(settings, c"accent".as_ptr() as *const u8),
        16,
    );
    if acc == "custom" {
        vis.accent = unpack_rgb((api.data_get_int)(settings, c"acolor".as_ptr() as *const u8));
        vis.accent_set = true;
    } else if let Ok(i) = acc.parse::<usize>() {
        if let Some((_, c)) = render::ACCENTS.get(i) {
            vis.accent = *c;
            vis.accent_set = true;
        }
    }
    *s.vis.lock().unwrap_or_else(|p| p.into_inner()) = vis;
}

/// プロパティ UI。キー/ボタンは接続中デバイスのレイアウトから生成する
/// (キーボード未接続時は既知の SU66 レイアウトを使う)。
#[cfg(windows)]
unsafe extern "C" fn get_properties(d: *mut c_void) -> *mut c_void {
    let Some(api) = ffi::resolve() else {
        return core::ptr::null_mut();
    };
    let s = &mut *(d as *mut Src);
    let snap = live::shared().lock().map(|l| l.clone()).unwrap_or_default();
    let props = (api.props_create)();
    // 説明文は OBS ロケールから (未翻訳は英語フォールバック)。ポインタは
    // OBS 所有か static なので lt() の結果をそのまま渡せる。
    let addb = |p: *mut c_void, name: &str, desc: *const u8| {
        let n = cs(name);
        (api.props_add_bool)(p, n.as_ptr() as *const u8, desc);
    };
    addb(props, "title", lt(c"devhub.title", c"Show title"));
    addb(props, "labels", lt(c"devhub.labels", c"Show key/button names"));
    // アクセント色。
    {
        let n = cs("accent");
        let l = (api.props_add_list)(
            props,
            n.as_ptr() as *const u8,
            lt(c"devhub.accent", c"Accent color"),
            ffi::OBS_COMBO_LIST,
            ffi::OBS_COMBO_FMT_STR,
        );
        // 既定: Device HUB アプリの「OBS オーバーレイ」画面で選んだ色。
        (api.prop_list_add_str)(
            l,
            lt(c"devhub.accent.app", c"Follow Device HUB app"),
            c"app".as_ptr() as *const u8,
        );
        for (i, (_, _)) in render::ACCENTS.iter().enumerate() {
            let (kk, fb): (CString, CString) = (
                cs(&format!("devhub.acc{i}")),
                cs(render::ACCENTS[i].0),
            );
            (api.prop_list_add_str)(
                l,
                lt(&kk, &fb),
                cs(&i.to_string()).as_ptr() as *const u8,
            );
        }
        (api.prop_list_add_str)(
            l,
            lt(c"devhub.accent.custom", c"Custom"),
            c"custom".as_ptr() as *const u8,
        );
        let n2 = cs("acolor");
        (api.props_add_color)(
            props,
            n2.as_ptr() as *const u8,
            lt(c"devhub.acolor", c"Custom accent color"),
        );
    }
    // ---- 全設定を OBS だけで行うモード (Device HUB アプリ不要・OSS 配布向け) ----
    addb(
        props,
        "obs_cfg",
        lt(
            c"devhub.obsall",
            c"Configure everything in OBS (ignore app settings)",
        ),
    );
    {
        let g = (api.props_create)();
        let addl = |name: &str,
                    desc: *const u8,
                    opts: &[(&std::ffi::CStr, &std::ffi::CStr, &str)]| {
            let n = cs(name);
            let l = (api.props_add_list)(
                g,
                n.as_ptr() as *const u8,
                desc,
                ffi::OBS_COMBO_LIST,
                ffi::OBS_COMBO_FMT_STR,
            );
            for (key, fb, val) in opts {
                (api.prop_list_add_str)(l, lt(key, fb), cs(val).as_ptr() as *const u8);
            }
        };
        let addgb = |name: &str, desc: *const u8| {
            let n = cs(name);
            (api.props_add_bool)(g, n.as_ptr() as *const u8, desc);
        };
        let addfs = |name: &str, desc: *const u8, min: f64, max: f64, step: f64| {
            let n = cs(name);
            (api.props_add_float_slider)(g, n.as_ptr() as *const u8, desc, min, max, step);
        };
        let addis = |name: &str, desc: *const u8, min: i32, max: i32, step: i32| {
            let n = cs(name);
            (api.props_add_int_slider)(g, n.as_ptr() as *const u8, desc, min, max, step);
        };
        addl(
            "theme",
            lt(c"devhub.theme", c"Theme"),
            &[
                (c"devhub.theme.glass", c"Glass", "glass"),
                (c"devhub.theme.light", c"Light", "light"),
                (c"devhub.theme.clear", c"Clear", "clear"),
                (c"devhub.theme.neon", c"Neon", "neon"),
            ],
        );
        if s.mode != render::Mode::Kb {
            addl(
                "mstyle",
                lt(c"devhub.mstyle", c"Mouse style"),
                &[
                    (c"devhub.mstyle.both", c"Shape + bars", "both"),
                    (c"devhub.mstyle.shape", c"Shape only", "shape"),
                    (c"devhub.mstyle.bars", c"Bars only", "bars"),
                ],
            );
        }
        if s.mode == render::Mode::Both {
            addl(
                "arrange",
                lt(c"devhub.arrange", c"Arrange"),
                &[
                    (c"devhub.arrange.side", c"Side by side", "side"),
                    (c"devhub.arrange.stack", c"Stacked", "stack"),
                    (c"devhub.arrange.swap", c"Swapped", "swap"),
                ],
            );
        }
        addl(
            "fill",
            lt(c"devhub.fill", c"Fill direction"),
            &[
                (c"devhub.fill.bottom", c"Bottom-up", "bottom"),
                (c"devhub.fill.top", c"Top-down", "top"),
                (c"devhub.fill.fade", c"Fade", "fade"),
            ],
        );
        addgb("numbers", lt(c"devhub.numbers", c"Show depth values"));
        addgb("num_pct", lt(c"devhub.num_pct", c"Show values as %"));
        addgb("badges", lt(c"devhub.badges", c"Show AP/RT badges"));
        addgb("tline", lt(c"devhub.tline", c"Show trigger line"));
        addgb("shadow", lt(c"devhub.shadow", c"Show shadows"));
        addgb("pcust", lt(c"devhub.pcust", c"Custom pressed color"));
        {
            let n = cs("pcolor");
            (api.props_add_color)(
                g,
                n.as_ptr() as *const u8,
                lt(c"devhub.pcolor", c"Pressed color"),
            );
        }
        addfs("oscale", lt(c"devhub.oscale", c"Render scale"), 0.75, 3.0, 0.25);
        addfs("ksize", lt(c"devhub.ksize", c"Key size"), 0.7, 1.4, 0.05);
        addfs("radius", lt(c"devhub.radius", c"Corner radius"), 0.0, 2.0, 0.1);
        addfs("opad", lt(c"devhub.opad", c"Panel padding"), 0.5, 2.0, 0.1);
        addis("palpha", lt(c"devhub.palpha", c"Panel opacity %"), 0, 100, 5);
        addis("glow", lt(c"devhub.glow", c"Press glow %"), 0, 200, 10);
        {
            let n = cs("ttext");
            (api.props_add_text)(
                g,
                n.as_ptr() as *const u8,
                lt(c"devhub.ttext", c"Title text (empty = device name)"),
                0,
            );
        }
        let n = cs("detail");
        (api.props_add_group)(
            props,
            n.as_ptr() as *const u8,
            lt(
                c"devhub.detail",
                c"Appearance (enabled by the toggle above)",
            ),
            ffi::OBS_GROUP_NORMAL,
            g,
        );
    }
    if s.mode != render::Mode::Mouse {
        // キー単位の表示選択 (チェックしたキーだけ表示。全未チェック = すべて表示)。
        let fallback = devhub_core::registry::su66max_layout();
        let layout: &[devhub_core::model::LayoutKey] = snap
            .kb
            .as_ref()
            .map(|k| k.layout.as_slice())
            .unwrap_or(&fallback);
        let g = (api.props_create)();
        for (i, k) in layout.iter().enumerate() {
            let n = cs(&format!("sk{i}"));
            let dd = cs(&k.label);
            (api.props_add_bool)(g, n.as_ptr() as *const u8, dd.as_ptr() as *const u8);
        }
        let n = cs("keys");
        (api.props_add_group)(
            props,
            n.as_ptr() as *const u8,
            lt(c"devhub.keys", c"Keys to show (unchecked = all)"),
            ffi::OBS_GROUP_NORMAL,
            g,
        );
    }
    if s.mode != render::Mode::Kb {
        match &snap.mouse {
            Some(m) if !m.names.is_empty() => {
                let g = (api.props_create)();
                for (i, name) in m.names.iter().enumerate() {
                    let (n, dd) = (cs(&format!("sb{i}")), cs(name));
                    (api.props_add_bool)(g, n.as_ptr() as *const u8, dd.as_ptr() as *const u8);
                }
                let n = cs("btns");
                (api.props_add_group)(
                    props,
                    n.as_ptr() as *const u8,
                    lt(c"devhub.btns", c"Buttons to show (unchecked = all)"),
                    ffi::OBS_GROUP_NORMAL,
                    g,
                );
            }
            _ => {
                let n = cs("btn_note");
                (api.props_add_text)(
                    props,
                    n.as_ptr() as *const u8,
                    lt(
                        c"devhub.btn.note",
                        c"No mouse connected — buttons appear once connected",
                    ),
                    ffi::OBS_TEXT_INFO,
                );
            }
        }
    }
    props
}

#[cfg(windows)]
unsafe extern "C" fn video_render(d: *mut c_void, _e: *mut c_void) {
    let Some(api) = ffi::resolve() else {
        logline("video_render: resolve() failed");
        return;
    };
    let s = &mut *(d as *mut Src);
    // 新しいデータ世代があれば補間のターゲットとして保持する。
    let (cfg, cfg_gen) = config::current();
    let cfg_changed = cfg_gen != s.last_cfg;
    let new_data = {
        match live::shared().lock() {
            Ok(l) => {
                if l.seq != s.last_seq {
                    s.live = l.clone();
                    s.last_seq = l.seq;
                    true
                } else {
                    false
                }
            }
            Err(_) => {
                s.last_seq = u64::MAX;
                false
            }
        }
    };
    // 毎フレーム補間を進め、動いていれば焼き直す — ポーリング (~15-25Hz)
    // より描画 (~60fps) が速いので、これで紙芝居を解消する。
    let now = std::time::Instant::now();
    let dt = now
        .duration_since(s.last_tick)
        .as_secs_f32()
        .clamp(0.0, 0.1);
    s.last_tick = now;
    let moving = s.advance(dt);
    if new_data || cfg_changed || moving || s.pix.is_none() {
        let vis = s.merge_vis(&cfg);
        let disp = s.smoothed_live();
        let pix = render::draw(&disp, s.font.as_ref(), s.mode, &vis);
        // サイズが変わったらテクスチャを作り直す。
        if s.pix.as_ref().is_some_and(|o| o.w != pix.w || o.h != pix.h) && !s.tex.is_null() {
            (api.tex_destroy)(s.tex);
            s.tex = core::ptr::null_mut();
        }
        s.pix = Some(pix);
        s.last_cfg = cfg_gen;
        s.dirty = true;
    }
    let Some(pix) = &s.pix else {
        s.first_render.get_or_insert_with(|| {
            logline("video_render: pix empty");
            true
        });
        return;
    };
    if s.tex.is_null() {
        s.tex = (api.tex_create)(
            pix.w,
            pix.h,
            ffi::GS_RGBA,
            1,
            core::ptr::null(),
            ffi::GS_DYNAMIC,
        );
        if s.tex.is_null() {
            logline("video_render: tex_create failed");
            return;
        }
        logline("video_render: tex created");
        s.dirty = true;
    }
    if s.dirty {
        (api.tex_set_image)(s.tex, pix.buf.as_ptr(), pix.w * 4, false);
        s.dirty = false;
    }
    (api.blend_push)();
    (api.blend_function)(ffi::GS_BLEND_SRCALPHA, ffi::GS_BLEND_INVSRCALPHA);
    let effect = (api.base_effect)(ffi::OBS_EFFECT_DEFAULT);
    // libobs 正規パターン: gs_effect_loop の pass 内で obs_source_draw。
    if let (Some(eff_loop), Some(draw)) = (api.effect_loop, api.source_draw) {
        if !effect.is_null() {
            let mut first = true;
            while eff_loop(effect, c"Draw".as_ptr() as *const u8) {
                draw(s.tex, 0, 0, 0, 0, false);
                if first {
                    first = false;
                    if s.first_render.is_none() {
                        s.first_render = Some(true);
                        logline("video_render: effect_loop+source_draw used");
                    }
                }
            }
        }
        (api.blend_pop)();
        return;
    }
    if !effect.is_null() {
        let tech = (api.effect_technique)(effect, c"Draw".as_ptr() as *const u8);
        let param = (api.effect_param)(effect, c"image".as_ptr() as *const u8);
        if s.first_render.is_none() {
            logline(&format!(
                "video_render: effect={effect:?} tech={tech:?} param={param:?} tex={:?}",
                s.tex
            ));
        }
        if tech.is_null() || param.is_null() {
            (api.blend_pop)();
            return;
        }
        (api.effect_set_texture)(param, s.tex);
        let passes = (api.technique_begin)(tech);
        for p in 0..passes {
            if (api.technique_begin_pass)(tech, p) {
                (api.draw_sprite)(s.tex, 0, pix.w, pix.h);
                (api.technique_end_pass)(tech);
            }
        }
        (api.technique_end)(tech);
        if s.first_render.is_none() {
            s.first_render = Some(true);
            logline(&format!(
                "video_render: drew {}x{} passes={passes}",
                pix.w, pix.h
            ));
        }
    } else {
        s.first_render.get_or_insert_with(|| {
            logline("video_render: effect null");
            true
        });
    }
    (api.blend_pop)();
}

/// `obs_register_source_s` は構造体をコピーするのでローカル生成でよい
/// (生ポインタを含むので static にはできない)。
#[cfg(windows)]
fn source_info(
    id: &'static std::ffi::CStr,
    get_name: unsafe extern "C" fn(*mut c_void) -> *const u8,
    create: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void,
) -> ffi::ObsSourceInfo {
    ffi::ObsSourceInfo {
        id: id.as_ptr() as *const u8,
        typ: ffi::OBS_SOURCE_TYPE_INPUT,
        output_flags: ffi::OBS_SOURCE_VIDEO | ffi::OBS_SOURCE_CUSTOM_DRAW,
        get_name: Some(get_name),
        create: Some(create),
        destroy: Some(destroy),
        get_width: Some(get_width),
        get_height: Some(get_height),
        get_defaults: Some(get_defaults),
        get_properties: Some(get_properties),
        update: Some(update),
        activate: None,
        deactivate: Some(deactivate),
        show: None,
        hide: None,
        video_tick: None,
        video_render: Some(video_render),
    }
}

// ---------------- モジュールエントリ ----------------
// DEVHUB: OBS プラグインのエクスポートは Windows 専リンク要件が無ければ割愛。
// macOS では OBS プラグインを提供しない (ffi.rs も cfg(windows))。

#[cfg(windows)]
#[no_mangle]
pub extern "C" fn obs_module_set_pointer(m: *mut c_void) {
    MODULE.store(m, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(windows)]
#[no_mangle]
pub extern "C" fn obs_current_module() -> *mut c_void {
    MODULE.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(windows)]
#[no_mangle]
pub extern "C" fn obs_module_ver() -> u32 {
    match ffi::resolve() {
        Some(api) => ffi::host_semver(api),
        None => 31 << 24,
    }
}

#[cfg(windows)]
#[no_mangle]
pub extern "C" fn obs_module_name() -> *const u8 {
    c"devhub-obs".as_ptr() as *const u8
}

#[cfg(windows)]
#[no_mangle]
pub extern "C" fn obs_module_description() -> *const u8 {
    unsafe {
        lt(
            c"devhub.desc",
            c"Device HUB: live magnetic key/button depth display",
        )
    }
}

#[cfg(windows)]
#[no_mangle]
pub extern "C" fn obs_module_author() -> *const u8 {
    c"Device HUB".as_ptr() as *const u8
}

#[cfg(windows)]
#[no_mangle]
pub extern "C" fn obs_module_load() -> bool {
    let Some(api) = ffi::resolve() else {
        return false;
    };
    let infos = [
        source_info(c"devhub_live", name_both, create_both),
        source_info(c"devhub_live_kb", name_kb, create_kb),
        source_info(c"devhub_live_mouse", name_mouse, create_mouse),
    ];
    unsafe {
        for info in &infos {
            (api.register_source_s)(info, core::mem::size_of::<ffi::ObsSourceInfo>());
        }
    }
    true
}

#[cfg(windows)]
#[no_mangle]
pub extern "C" fn obs_module_unload() {}
