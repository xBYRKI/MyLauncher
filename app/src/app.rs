//! Fenster, Zustand und Ansichten. Langsames läuft auf der Tokio-Runtime und meldet
//! sich per [`Msg`] zurück; die Oberfläche selbst blockiert nie.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, text::LayoutJob, Align, Align2, Color32, CornerRadius, Id, Layout, Margin, Pos2, Rect,
    ResizeDirection, RichText, Sense, Stroke, StrokeKind, TextFormat, TextureHandle,
    TextureOptions, Ui, Vec2, ViewportCommand,
};
use launcher_core::config::{self, LauncherConfig, Settings};
use launcher_core::modpack::{ChangeKind, PackPlan, PackStatus, Progress};
use launcher_core::ping::{self, MotdPart, ServerStatus};
use launcher_core::prism::{Account, Instance, Prism};
use launcher_core::update::{self, UpdateStatus};
use notify::Watcher;
use serde_json::{json, Value};

use crate::backend::{self, VERSION};
use crate::theme::{self, badge, muted, panel, panel_with, section, title, Icon, Kind, McButton};

const TITLE_H: f32 = 36.0;
const SIDEBAR_W: f32 = 236.0;
/// Hintergrundprüfung auf neue Modpack- und Launcher-Versionen.
const CHECK_INTERVAL: Duration = Duration::from_secs(15 * 60);
/// Fokuswechsel lösen höchstens so oft eine Prüfung aus.
const FOCUS_CHECK_MIN: Duration = Duration::from_secs(5 * 60);
const PING_INTERVAL: Duration = Duration::from_secs(60);

pub fn run() -> eframe::Result {
    let brand = config::load_launcher_config().brand.name;
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(&brand)
        .with_inner_size([1180.0, 740.0])
        .with_min_inner_size([900.0, 600.0])
        .with_decorations(false);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../icons/icon.png")) {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport,
        centered: true,
        ..Default::default()
    };
    eframe::run_native(&brand, options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

// ---------- Nachrichten ----------

#[derive(Clone, Copy, PartialEq)]
enum Tone {
    Ok,
    Error,
}

enum Msg {
    Prism(Prism, Vec<Account>),
    Instances(Vec<Instance>),
    InstancesChanged,
    Packs(Vec<PackStatus>),
    PackProgress(String, Progress),
    PackDone(String),
    PackPlan(String, backend::Res<PackPlan>),
    News(Vec<Value>),
    /// Server-Konfiguration geladen – `true`, wenn sie sich geändert hat.
    RemoteConfig(bool),
    Server(usize, ServerStatus),
    Update(UpdateStatus),
    UpdateProgress(update::Progress),
    UpdateDone(backend::Res<String>),
    LaunchDone(String),
    Launched,
    Toast(Tone, String),
    Image(String, backend::Res<Vec<u8>>),
    Picked { exe: bool, path: Option<String> },
    Quit,
}

#[derive(Clone)]
struct Tx {
    tx: Sender<Msg>,
    ctx: egui::Context,
}

impl Tx {
    fn send(&self, msg: Msg) {
        let _ = self.tx.send(msg);
        self.ctx.request_repaint();
    }
}

// ---------- Bilder ----------

enum Img {
    Loading,
    Ready(TextureHandle),
    Failed,
}

fn decode(ctx: &egui::Context, key: &str, bytes: &[u8], pixelated: bool) -> Option<TextureHandle> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    let options = if pixelated { TextureOptions::NEAREST } else { TextureOptions::LINEAR };
    Some(ctx.load_texture(key, color, options))
}

/// `data:image/png;base64,…` → Bytes
fn data_url_bytes(url: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let (_, b64) = url.split_once("base64,")?;
    base64::engine::general_purpose::STANDARD.decode(b64.trim()).ok()
}

// ---------- Zustand ----------

#[derive(Clone, Copy, PartialEq)]
enum View {
    Home,
    Instances,
    Packs,
    Settings,
}

struct Toast {
    tone: Tone,
    text: String,
    until: Instant,
}

/// Textfelder der Einstellungen – gespeichert wird beim Verlassen des Feldes.
#[derive(Default)]
struct Form {
    prism_exe: String,
    prism_data: String,
}

impl Form {
    fn from(s: &Settings) -> Self {
        Self {
            prism_exe: s.prism_exe.clone(),
            prism_data: s.prism_data_dir.clone(),
        }
    }
}

pub struct App {
    rt: tokio::runtime::Runtime,
    tx: Tx,
    rx: Receiver<Msg>,

    cfg: LauncherConfig,
    settings: Settings,
    form: Form,
    prism: Option<Prism>,
    accounts: Vec<Account>,
    instances: Arc<Vec<Instance>>,
    packs: Arc<Vec<PackStatus>>,
    news: Option<Arc<Vec<Value>>>,
    servers: Vec<Option<ServerStatus>>,

    /// Laufende Pack-Vorgänge (Manifest-URL) und ihr letzter Fortschritt
    busy: HashSet<String>,
    progress: HashMap<String, Progress>,
    /// Instanzen, deren Start gerade angestoßen wurde
    launching: HashSet<String>,

    update: Option<UpdateStatus>,
    update_checking: bool,
    update_quiet: bool,
    update_busy: bool,
    update_progress: Option<update::Progress>,

    view: View,
    search: String,
    plan: Option<(PackStatus, PackPlan)>,
    toasts: Vec<Toast>,

    images: HashMap<String, Img>,
    bg: TextureHandle,
    logo: Option<TextureHandle>,

    watcher: Option<notify::RecommendedWatcher>,
    instances_dirty: Option<Instant>,
    last_check: Instant,
    last_ping: Instant,
    was_focused: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = cc.egui_ctx.clone();
        theme::install(&ctx);
        let (tx, rx) = mpsc::channel();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Tokio-Runtime");
        let settings = config::load_settings();
        let cfg = config::load_launcher_config();
        let mut app = Self {
            rt,
            tx: Tx { tx, ctx: ctx.clone() },
            rx,
            form: Form::from(&settings),
            servers: vec![None; cfg.servers.len()],
            cfg,
            settings,
            prism: None,
            accounts: vec![],
            instances: Arc::default(),
            packs: Arc::default(),
            news: None,
            busy: HashSet::new(),
            progress: HashMap::new(),
            launching: HashSet::new(),
            update: None,
            update_checking: false,
            update_quiet: false,
            update_busy: false,
            update_progress: None,
            view: View::Home,
            search: String::new(),
            plan: None,
            toasts: vec![],
            images: HashMap::new(),
            bg: theme::deepslate(&ctx),
            logo: decode(&ctx, "logo", include_bytes!("../assets/logo.png"), false),
            watcher: None,
            instances_dirty: None,
            last_check: Instant::now(),
            last_ping: Instant::now(),
            was_focused: true,
        };
        app.rewatch();
        app.refresh_remote();
        app.refresh_prism();
        app.load_news();
        app.load_packs();
        app.ping_servers();
        if app.settings.auto_update_launcher {
            app.check_update(true);
        }
        app
    }

    fn spawn<F>(&self, fut: F)
    where
        F: Future<Output = Msg> + Send + 'static,
    {
        let tx = self.tx.clone();
        self.rt.spawn(async move { tx.send(fut.await) });
    }

    fn toast(&mut self, tone: Tone, text: impl Into<String>) {
        let secs = if tone == Tone::Error { 8 } else { 4 };
        self.toasts.push(Toast {
            tone,
            text: text.into(),
            until: Instant::now() + Duration::from_secs(secs),
        });
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
    }

    fn report(&mut self, r: backend::Res<()>) {
        if let Err(e) = r {
            self.toast(Tone::Error, e);
        }
    }

    // ---------- Laden ----------

    /// Instanzordner beobachten, damit Änderungen in Prism sofort erscheinen.
    fn rewatch(&mut self) {
        let tx = self.tx.clone();
        let dir = backend::current_prism().instances_dir;
        self.watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if res.is_ok() {
                tx.send(Msg::InstancesChanged);
            }
        })
        .ok()
        .and_then(|mut w| {
            w.watch(&dir, notify::RecursiveMode::NonRecursive)
                .ok()
                .map(|_| w)
        });
    }

    fn refresh_prism(&self) {
        self.spawn(async {
            let (p, accounts) = backend::prism_and_accounts().await;
            Msg::Prism(p, accounts)
        });
        self.load_instances();
    }

    fn load_instances(&self) {
        self.spawn(async { Msg::Instances(backend::instances().await) });
    }

    fn load_packs(&self) {
        self.spawn(async { Msg::Packs(backend::packs().await) });
    }

    fn load_news(&self) {
        self.spawn(async { Msg::News(backend::news().await) });
    }

    /// Packs, News und Server vom eigenen Server holen (falls `remoteConfig` gesetzt ist).
    fn refresh_remote(&self) {
        self.spawn(async { Msg::RemoteConfig(backend::refresh_remote_config().await) });
    }

    fn ping_servers(&mut self) {
        self.last_ping = Instant::now();
        for (i, srv) in self.cfg.servers.iter().enumerate() {
            let address = srv.address.clone();
            self.spawn(async move { Msg::Server(i, ping::status(&address).await) });
        }
    }

    /// Neue Modpack- und Launcher-Versionen von selbst bemerken.
    fn background_check(&mut self) {
        self.last_check = Instant::now();
        self.refresh_remote();
        self.load_packs();
        if self.settings.auto_update_launcher {
            self.check_update(true);
        }
    }

    fn save(&mut self, patch: Value) {
        match backend::save_settings(&patch) {
            Ok(s) => {
                self.form = Form::from(&s);
                self.settings = s;
                self.rewatch();
                self.refresh_prism();
            }
            Err(e) => self.toast(Tone::Error, e),
        }
    }

    // ---------- Aktionen ----------

    fn pack_for_instance(&self, inst: &Instance) -> Option<PackStatus> {
        let managed = inst.managed_by.as_ref()?;
        self.packs
            .iter()
            .find(|p| p.id == managed.pack_id && p.error.is_none())
            .cloned()
    }

    fn launch(&mut self, inst: &Instance, server: Option<String>, done: String) {
        if !self.launching.insert(inst.id.clone()) {
            return;
        }
        let tx = self.tx.clone();
        let id = inst.id.clone();
        self.rt.spawn(async move {
            match backend::launch(id.clone(), server).await {
                Ok(()) => {
                    tx.send(Msg::Toast(Tone::Ok, done));
                    tx.send(Msg::Launched);
                }
                Err(e) => tx.send(Msg::Toast(Tone::Error, e)),
            }
            // Button kurz gesperrt lassen, sonst startet ein Doppelklick zweimal
            tokio::time::sleep(Duration::from_millis(1200)).await;
            tx.send(Msg::LaunchDone(id));
        });
    }

    fn play_instance(&mut self, inst: &Instance) {
        if let Some(pack) = self.pack_for_instance(inst) {
            return self.play_pack(&pack, false);
        }
        let done = format!("{} wird über Prism gestartet", inst.name);
        self.launch(inst, None, done);
    }

    fn progress_cb(&self, source: &str) -> impl Fn(Progress) + Send + Sync + 'static {
        let tx = self.tx.clone();
        let source = source.to_string();
        move |p| tx.send(Msg::PackProgress(source.clone(), p))
    }

    fn start_pack_task(&mut self, pack: &PackStatus) -> bool {
        if !self.busy.insert(pack.source.clone()) {
            return false;
        }
        self.progress.insert(
            pack.source.clone(),
            Progress { phase: "check", done: 0, total: 0, file: String::new(), bytes: 0, total_bytes: 0 },
        );
        true
    }

    fn play_pack(&mut self, pack: &PackStatus, install_only: bool) {
        if !self.start_pack_task(pack) {
            return;
        }
        let tx = self.tx.clone();
        let on_progress = self.progress_cb(&pack.source);
        let source = pack.source.clone();
        let name = pack.name.clone();
        self.rt.spawn(async move {
            let result = if install_only {
                backend::sync_pack(&source, on_progress).await
            } else {
                backend::play_pack(&source, on_progress).await
            };
            match result {
                Ok(r) => {
                    let changes = if r.downloaded + r.removed > 0 {
                        format!(" ({} geladen, {} entfernt)", r.downloaded, r.removed)
                    } else {
                        String::new()
                    };
                    let text = if install_only {
                        format!("{name} ist aktuell{changes}")
                    } else {
                        format!("{name} wird gestartet{changes}")
                    };
                    tx.send(Msg::Toast(Tone::Ok, text));
                    if !install_only {
                        tx.send(Msg::Launched);
                    }
                }
                Err(e) => tx.send(Msg::Toast(Tone::Error, e)),
            }
            tx.send(Msg::PackDone(source));
        });
    }

    /// Prüft das Pack (SHA1 über alle verwalteten Dateien) und zeigt das Ergebnis,
    /// ohne etwas zu verändern.
    fn review_pack(&mut self, pack: &PackStatus) {
        if !self.start_pack_task(pack) {
            return;
        }
        let on_progress = self.progress_cb(&pack.source);
        let source = pack.source.clone();
        self.spawn(async move {
            let plan = backend::pack_plan(&source, on_progress).await;
            Msg::PackPlan(source, plan)
        });
    }

    /// `quiet` = Hintergrundprüfung: meldet sich nur, wenn eine neue Version auftaucht.
    fn check_update(&mut self, quiet: bool) {
        if self.update_busy || self.update_checking {
            return;
        }
        self.update_checking = true;
        self.update_quiet = quiet;
        self.spawn(async { Msg::Update(backend::update_status().await) });
    }

    fn install_update(&mut self) {
        let Some(u) = self.update.as_ref().filter(|u| u.available) else {
            return;
        };
        if self.update_busy {
            return;
        }
        self.update_busy = true;
        self.update_progress = Some(update::Progress { phase: "download", bytes: 0, total_bytes: u.size });
        let tx = self.tx.clone();
        self.spawn(async move {
            let r = backend::update_install(move |p| tx.send(Msg::UpdateProgress(p))).await;
            Msg::UpdateDone(r)
        });
    }

    fn pick(&self, exe: bool) {
        self.spawn(async move {
            let path = if exe { backend::pick_file().await } else { backend::pick_dir().await };
            Msg::Picked { exe, path }
        });
    }

    // ---------- Nachrichten verarbeiten ----------

    fn handle(&mut self, ctx: &egui::Context, msg: Msg) {
        match msg {
            Msg::Prism(p, accounts) => {
                self.prism = Some(p);
                self.accounts = accounts;
            }
            Msg::Instances(list) => self.instances = Arc::new(list),
            Msg::InstancesChanged => self.instances_dirty = Some(Instant::now()),
            Msg::Packs(list) => self.packs = Arc::new(list),
            Msg::PackProgress(source, p) => {
                if self.busy.contains(&source) {
                    self.progress.insert(source, p);
                }
            }
            Msg::PackDone(source) => {
                self.busy.remove(&source);
                self.progress.remove(&source);
                self.load_packs();
                self.load_instances();
            }
            Msg::PackPlan(source, plan) => {
                self.busy.remove(&source);
                self.progress.remove(&source);
                match plan {
                    Ok(plan) => {
                        if let Some(pack) = self.packs.iter().find(|p| p.source == source) {
                            self.plan = Some((pack.clone(), plan));
                        }
                    }
                    Err(e) => self.toast(Tone::Error, e),
                }
            }
            Msg::News(items) => self.news = Some(Arc::new(items)),
            Msg::RemoteConfig(changed) => {
                if changed {
                    self.cfg = config::load_launcher_config();
                    self.servers = vec![None; self.cfg.servers.len()];
                    self.ping_servers();
                    self.load_news();
                    self.load_packs();
                }
            }
            Msg::Server(i, s) => {
                if let Some(slot) = self.servers.get_mut(i) {
                    *slot = Some(s);
                }
            }
            Msg::Update(u) => {
                self.update_checking = false;
                let known = self.update.as_ref().filter(|u| u.available).map(|u| u.version.clone());
                if u.available && Some(&u.version) != known.as_ref() {
                    self.toast(Tone::Ok, format!("Launcher-Update {} verfügbar – unter „Einstellungen“ installieren.", u.version));
                } else if !self.update_quiet && !u.available && u.error.is_none() && u.configured {
                    self.toast(Tone::Ok, format!("{} {} ist aktuell.", self.cfg.brand.name, u.current));
                }
                self.update = Some(u);
            }
            Msg::UpdateProgress(p) => self.update_progress = Some(p),
            Msg::UpdateDone(r) => match r {
                Ok(version) => {
                    self.toast(Tone::Ok, format!("Installer für {version} gestartet – der Launcher beendet sich jetzt."));
                    // Der Installer kann die laufende Exe nicht ersetzen – also Platz machen.
                    self.spawn(async {
                        tokio::time::sleep(Duration::from_millis(1200)).await;
                        Msg::Quit
                    });
                }
                Err(e) => {
                    self.toast(Tone::Error, e);
                    self.update_busy = false;
                    self.update_progress = None;
                }
            },
            Msg::LaunchDone(id) => {
                self.launching.remove(&id);
            }
            Msg::Launched => match self.settings.after_launch.as_str() {
                "close" => ctx.send_viewport_cmd(ViewportCommand::Close),
                "minimize" => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
                _ => {}
            },
            Msg::Toast(tone, text) => self.toast(tone, text),
            Msg::Image(key, bytes) => {
                let tex = bytes.ok().and_then(|b| decode(ctx, &key, &b, key.contains("mc-heads")));
                self.images.insert(key, tex.map(Img::Ready).unwrap_or(Img::Failed));
            }
            Msg::Picked { exe, path } => {
                if let Some(path) = path {
                    let key = if exe { "prismExe" } else { "prismDataDir" };
                    self.save(json!({ key: path }));
                }
            }
            Msg::Quit => ctx.send_viewport_cmd(ViewportCommand::Close),
        }
    }

    // ---------- Bilder ----------

    fn data_image(&mut self, ctx: &egui::Context, key: &str, url: &str) -> Option<TextureHandle> {
        let entry = self.images.entry(key.to_string()).or_insert_with(|| {
            data_url_bytes(url)
                .and_then(|b| decode(ctx, key, &b, true))
                .map(Img::Ready)
                .unwrap_or(Img::Failed)
        });
        match entry {
            Img::Ready(t) => Some(t.clone()),
            _ => None,
        }
    }

    fn remote_image(&mut self, url: &str) -> Option<TextureHandle> {
        match self.images.get(url) {
            Some(Img::Ready(t)) => Some(t.clone()),
            Some(_) => None,
            None => {
                self.images.insert(url.to_string(), Img::Loading);
                let u = url.to_string();
                self.spawn(async move {
                    let bytes = backend::fetch_image(&u).await;
                    Msg::Image(u, bytes)
                });
                None
            }
        }
    }
}

// ---------- Formatierung ----------

fn fmt_playtime(sec: u64) -> String {
    if sec == 0 {
        return "Noch nie gespielt".into();
    }
    let (h, m) = (sec / 3600, (sec % 3600) / 60);
    if h > 0 {
        format!("{h} Std. {m} Min.")
    } else {
        format!("{m} Min.")
    }
}

/// Tage seit 1970 → (Jahr, Monat, Tag), nach Howard Hinnant.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn fmt_ago(ms: u64) -> String {
    if ms == 0 {
        return "—".into();
    }
    let s = launcher_core::now_millis().saturating_sub(ms) / 1000;
    match s {
        0..60 => "gerade eben".into(),
        60..3600 => format!("vor {} Min.", s / 60),
        3600..86_400 => format!("vor {} Std.", s / 3600),
        _ if s / 86_400 < 30 => {
            let d = s / 86_400;
            format!("vor {d} Tag{}", if d == 1 { "" } else { "en" })
        }
        _ => {
            let (y, m, d) = civil((ms / 86_400_000) as i64);
            format!("{d:02}.{m:02}.{y}")
        }
    }
}

const MONTHS: [&str; 12] = [
    "Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober", "November", "Dezember",
];

/// `2026-10-18` → `18. Oktober 2026`
fn fmt_date(iso: &str) -> String {
    let mut parts = iso.get(..10).unwrap_or(iso).split('-').map(|p| p.parse::<usize>().ok());
    match (parts.next().flatten(), parts.next().flatten(), parts.next().flatten()) {
        (Some(y), Some(m @ 1..=12), Some(d)) => format!("{d}. {} {y}", MONTHS[m - 1]),
        _ => iso.to_string(),
    }
}

fn fmt_bytes(n: u64) -> String {
    if n < 1024 * 1024 {
        format!("{:.0} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / 1024.0 / 1024.0)
    }
}

fn fmt_count(n: i64) -> String {
    // Tausenderpunkte wie im Deutschen
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push('.');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

fn loader_text(inst: &Instance) -> String {
    inst.loader
        .as_ref()
        .map(|l| format!("{} {}", l.name, l.version))
        .unwrap_or_else(|| "Vanilla".into())
}

fn progress_text(p: Option<&Progress>) -> String {
    let Some(p) = p else { return "Starte …".into() };
    match p.phase {
        "check" => format!("Prüfe Dateien … {}/{}", p.done, p.total),
        "download" if p.total_bytes > 0 => format!(
            "Lade {}/{} · {} / {}",
            p.done,
            p.total,
            fmt_bytes(p.bytes),
            fmt_bytes(p.total_bytes)
        ),
        "download" => format!("Lade {}/{}", p.done, p.total),
        "done" => "Fertig – starte Prism …".into(),
        _ => "Starte …".into(),
    }
}

fn progress_fraction(p: Option<&Progress>) -> f32 {
    let Some(p) = p else { return 0.0 };
    match p.phase {
        "done" => 1.0,
        "download" if p.total_bytes > 0 => p.bytes as f32 / p.total_bytes as f32,
        _ if p.total > 0 => p.done as f32 / p.total as f32,
        _ => 0.0,
    }
}

fn mc_color(name: &str) -> Option<Color32> {
    let hex = match name {
        "black" => 0x000000,
        "dark_blue" => 0x0000AA,
        "dark_green" => 0x00AA00,
        "dark_aqua" => 0x00AAAA,
        "dark_red" => 0xAA0000,
        "dark_purple" => 0xAA00AA,
        "gold" => 0xFFAA00,
        "gray" => 0xAAAAAA,
        "dark_gray" => 0x555555,
        "blue" => 0x5555FF,
        "green" => 0x55FF55,
        "aqua" => 0x55FFFF,
        "red" => 0xFF5555,
        "light_purple" => 0xFF55FF,
        "yellow" => 0xFFFF55,
        "white" => 0xFFFFFF,
        other => u32::from_str_radix(other.strip_prefix('#').filter(|h| h.len() == 6)?, 16).ok()?,
    };
    Some(Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8))
}

fn motd_job(parts: &[MotdPart], wrap: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap;
    job.wrap.max_rows = 2;
    for p in parts {
        let color = p.color.as_deref().and_then(mc_color).unwrap_or(theme::GRAY);
        job.append(
            &p.text,
            0.0,
            TextFormat {
                font_id: theme::mc_font(11.0),
                color,
                italics: p.italic,
                ..Default::default()
            },
        );
    }
    job
}

fn hue_of(text: &str) -> f32 {
    let x = text.chars().fold(0u32, |x, c| (x * 31 + c as u32) % 360);
    x as f32 / 360.0
}

// ---------- Bausteine ----------

/// Instanz-Icon aus Prism, sonst ein farbiger Block mit Kürzel.
fn block_icon(ui: &mut Ui, tex: Option<TextureHandle>, name: &str, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let p = ui.painter();
    if let Some(tex) = tex {
        p.rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(120));
        p.image(tex.id(), rect.shrink(2.0), Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        return;
    }
    let base: Color32 = egui::ecolor::Hsva::new(hue_of(name), 0.55, 0.55, 1.0).into();
    let light: Color32 = egui::ecolor::Hsva::new(hue_of(name), 0.45, 0.75, 1.0).into();
    let dark: Color32 = egui::ecolor::Hsva::new(hue_of(name), 0.6, 0.3, 1.0).into();
    p.extend(theme::bevel_shapes(rect, base, light, dark, true));
    let initials: String = name.chars().filter(|c| c.is_alphanumeric()).take(2).collect::<String>().to_uppercase();
    let initials = if initials.is_empty() { "?".into() } else { initials };
    theme::shadow_text(p, rect.center(), Align2::CENTER_CENTER, &initials, theme::mc_font(size * 0.36), theme::WHITE);
}

/// Seitenkopf: Titel links, Aktionen rechts.
fn view_head(ui: &mut Ui, heading: &str, sub: &str, actions: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            title(ui, heading, 24.0, theme::WHITE);
            muted(ui, sub);
        });
        ui.with_layout(Layout::right_to_left(Align::Center), actions);
    });
    ui.add_space(10.0);
}

fn empty(ui: &mut Ui, text: &str) {
    panel(ui, |ui| {
        muted(ui, text);
    });
}

fn win_button(ui: &mut Ui, icon: Icon, close: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(46.0, TITLE_H), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        let fill = if close { Color32::from_rgb(0xC4, 0x2B, 0x1C) } else { Color32::from_rgb(0x3A, 0x3A, 0x3E) };
        p.rect_filled(rect, CornerRadius::ZERO, fill);
    }
    let color = if resp.hovered() { theme::WHITE } else { theme::GRAY };
    let scale = 1.5;
    icon.paint(p, (rect.center() - Vec2::splat(4.0 * scale)).round(), scale, color);
    resp
}

// ---------- Ansichten ----------

impl App {
    fn ui_titlebar(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let bar = ui.interact(ui.max_rect(), Id::new("titlebar"), Sense::click_and_drag());
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        if bar.drag_started_by(egui::PointerButton::Primary) {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if bar.double_clicked() {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        }
        ui.horizontal_centered(|ui| {
            ui.add_space(12.0);
            if let Some(logo) = &self.logo {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                ui.painter().image(logo.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
            title(ui, &self.cfg.brand.name, 13.0, theme::WHITE);
            ui.label(RichText::new(format!("v{VERSION}")).font(theme::mc_font(10.0)).color(theme::DARK_GRAY));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if win_button(ui, Icon::Close, true).clicked() {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
                if win_button(ui, Icon::Max, false).clicked() {
                    ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
                }
                if win_button(ui, Icon::Min, false).clicked() {
                    ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
                }
            });
        });
    }

    fn ui_sidebar(&mut self, ui: &mut Ui) {
        let pending = self.packs.iter().filter(|p| p.error.is_none() && !p.up_to_date).count();
        let update_available = self.update.as_ref().is_some_and(|u| u.available);
        let nav = [
            (View::Home, "Start", Icon::Home, String::new(), false),
            (View::Instances, "Instanzen", Icon::Grid, count(self.instances.len()), false),
            (View::Packs, "Modpacks", Icon::Chest, count(self.packs.len()), pending > 0),
            (View::Settings, "Einstellungen", Icon::Gear, if update_available { "1".into() } else { String::new() }, update_available),
        ];
        let w = ui.available_width();
        for (view, label, icon, n, alert) in nav {
            let kind = if self.view == view { Kind::Selected } else { Kind::Stone };
            let resp = ui.add(McButton::new(label).icon(icon).kind(kind).width(w).height(36.0));
            if !n.is_empty() {
                let color = if alert { theme::GOLD } else { theme::GRAY };
                theme::shadow_text(ui.painter(), resp.rect.right_center() - Vec2::new(12.0, 0.0), Align2::RIGHT_CENTER, &n, theme::mc_font(12.0), color);
            }
            if resp.clicked() {
                self.view = view;
            }
        }

        ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
            // Der Panel-Rand unten zählt im Bottom-up-Layout nicht mit
            ui.add_space(12.0);
            self.ui_account(ui);
            ui.add_space(4.0);
            let found = self.prism.as_ref().is_some_and(|p| p.found);
            let resp = ui
                .horizontal(|ui| {
                    theme::dot(ui, found);
                    let text = if found { "Prism verbunden" } else { "Prism nicht gefunden" };
                    ui.add(egui::Label::new(RichText::new(text).font(theme::mc_font(11.0)).color(theme::GRAY)).sense(Sense::click()))
                })
                .inner
                .on_hover_text("Prism Launcher öffnen");
            if resp.clicked() {
                if found {
                    let r = backend::show_in_prism(None);
                    self.report(r);
                } else {
                    self.view = View::Settings;
                }
            }
        });
    }

    fn selected_account(&self) -> Option<&Account> {
        self.accounts
            .iter()
            .find(|a| a.name == self.settings.account)
            .or_else(|| self.accounts.iter().find(|a| a.active))
            .or_else(|| self.accounts.first())
    }

    fn ui_account(&mut self, ui: &mut Ui) {
        let acc = self.selected_account().cloned();
        let head = acc
            .as_ref()
            .filter(|a| !a.uuid.is_empty())
            .and_then(|a| self.remote_image(&format!("https://mc-heads.net/avatar/{}/64", a.uuid)));
        panel(ui, |ui| {
            ui.horizontal(|ui| {
                let name = acc.as_ref().map(|a| a.name.as_str()).unwrap_or("?");
                block_icon(ui, head, name, 36.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new("KONTO").font(theme::mc_font(10.0)).color(theme::DARK_GRAY));
                    let active = self.accounts.iter().find(|a| a.active).map(|a| a.name.clone());
                    let current = if self.accounts.is_empty() {
                        "Kein Konto – in Prism anmelden".to_string()
                    } else if self.settings.account.is_empty() {
                        active.clone().unwrap_or_else(|| "Prism-Standard".into())
                    } else {
                        self.settings.account.clone()
                    };
                    let mut choice = None;
                    egui::ComboBox::from_id_salt("account")
                        .width(ui.available_width())
                        .selected_text(current)
                        .show_ui(ui, |ui| {
                            let default = format!("{} (Standard)", active.as_deref().unwrap_or("Prism"));
                            if ui.selectable_label(self.settings.account.is_empty(), default).clicked() {
                                choice = Some(String::new());
                            }
                            for a in &self.accounts {
                                if ui.selectable_label(a.name == self.settings.account, &a.name).clicked() {
                                    choice = Some(a.name.clone());
                                }
                            }
                        });
                    if let Some(name) = choice {
                        self.save(json!({ "account": name }));
                    }
                });
            });
        });
    }

    // ----- Start -----

    fn ui_home(&mut self, ui: &mut Ui) {
        self.ui_hero(ui);
        ui.add_space(8.0);
        let wide = ui.available_width() > 760.0;
        if wide {
            ui.columns(2, |cols| {
                section(&mut cols[0], "Neuigkeiten");
                self.ui_news(&mut cols[0]);
                section(&mut cols[1], "Server");
                self.ui_servers(&mut cols[1]);
                section(&mut cols[1], "Zuletzt gespielt");
                self.ui_recent(&mut cols[1]);
            });
        } else {
            section(ui, "Server");
            self.ui_servers(ui);
            section(ui, "Zuletzt gespielt");
            self.ui_recent(ui);
            section(ui, "Neuigkeiten");
            self.ui_news(ui);
        }
    }

    fn ui_hero(&mut self, ui: &mut Ui) {
        let packs = self.packs.clone();
        let instances = self.instances.clone();
        let accent = theme::parse_hex(&self.cfg.brand.accent).unwrap_or(theme::GOLD);
        panel_with(ui, accent, |ui| {
            ui.add_space(6.0);
            if let Some(pack) = packs.iter().find(|p| p.error.is_none()) {
                let busy = self.busy.contains(&pack.source);
                ui.label(RichText::new("MODPACK").font(theme::mc_font(12.0)).color(accent));
                title(ui, &pack.name, 32.0, theme::WHITE);
                ui.horizontal_wrapped(|ui| {
                    badge(ui, &pack.minecraft, theme::GREEN);
                    badge(ui, &pack.loader, theme::GRAY);
                    match &pack.installed_version {
                        Some(_) if pack.up_to_date => badge(ui, &format!("v{} installiert", pack.version), theme::GREEN),
                        Some(v) => badge(ui, &format!("Update {v} → {}", pack.version), theme::GOLD),
                        None => badge(ui, "Nicht installiert", theme::GOLD),
                    };
                    if !pack.server.is_empty() {
                        badge(ui, &pack.server, theme::AQUA);
                    }
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let label = if pack.installed_version.is_some() { "SPIELEN" } else { "INSTALLIEREN & SPIELEN" };
                    if ui.add(McButton::play(label).big().enabled(!busy)).clicked() {
                        self.play_pack(pack, false);
                    }
                    if busy {
                        ui.add_space(8.0);
                        let p = self.progress.get(&pack.source);
                        theme::xp_bar(ui, progress_fraction(p), &progress_text(p));
                    }
                });
            } else if let Some(inst) = instances.first() {
                ui.label(RichText::new("WEITERSPIELEN").font(theme::mc_font(12.0)).color(accent));
                title(ui, &inst.name, 32.0, theme::WHITE);
                ui.horizontal_wrapped(|ui| {
                    badge(ui, &inst.mc_version, theme::GREEN);
                    badge(ui, &loader_text(inst), theme::GRAY);
                    badge(ui, &format!("Zuletzt {}", fmt_ago(inst.last_launch)), theme::GRAY);
                });
                ui.add_space(10.0);
                let busy = self.launching.contains(&inst.id);
                let label = if busy { "STARTET …" } else { "SPIELEN" };
                if ui.add(McButton::play(label).big().enabled(!busy)).clicked() {
                    self.play_instance(inst);
                }
            } else {
                let found = self.prism.as_ref().is_some_and(|p| p.found);
                ui.label(RichText::new("WILLKOMMEN").font(theme::mc_font(12.0)).color(accent));
                title(ui, &self.cfg.brand.name.clone(), 32.0, theme::WHITE);
                muted(
                    ui,
                    if found {
                        "Noch keine Instanzen. Lege eine in Prism an oder trage ein Modpack in launcher.config.json ein."
                    } else {
                        "Prism Launcher wurde nicht gefunden. Installiere Prism oder setze den Pfad in den Einstellungen."
                    },
                );
                ui.add_space(6.0);
                if found {
                    if ui.add(McButton::new("Prism öffnen").icon(Icon::External)).clicked() {
                        let r = backend::show_in_prism(None);
                        self.report(r);
                    }
                } else if ui.add(McButton::new("Einstellungen").icon(Icon::Gear)).clicked() {
                    self.view = View::Settings;
                }
            }
            ui.add_space(4.0);
        });
    }

    fn ui_news(&mut self, ui: &mut Ui) {
        let Some(items) = self.news.clone() else {
            return empty(ui, "Lade …");
        };
        if items.is_empty() {
            return empty(ui, "Keine Neuigkeiten.");
        }
        for n in items.iter().take(8) {
            let image = n["image"].as_str().filter(|u| u.starts_with("https://")).and_then(|u| self.remote_image(u));
            panel(ui, |ui| {
                if let Some(tex) = image {
                    let w = ui.available_width();
                    let size = tex.size_vec2();
                    let h = (w * size.y / size.x.max(1.0)).min(180.0);
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
                    ui.painter().image(tex.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                }
                if let Some(date) = n["date"].as_str().filter(|d| !d.is_empty()) {
                    ui.label(RichText::new(fmt_date(date)).font(theme::mc_font(10.0)).color(theme::DARK_GRAY));
                }
                title(ui, n["title"].as_str().unwrap_or(""), 15.0, theme::WHITE);
                if let Some(body) = n["body"].as_str() {
                    ui.label(body);
                }
                if let Some(url) = n["url"].as_str().filter(|u| !u.is_empty()) {
                    if ui.add(McButton::new("Mehr lesen").icon(Icon::External)).clicked() {
                        let r = backend::open_external(url);
                        self.report(r);
                    }
                }
            });
        }
    }

    fn ui_servers(&mut self, ui: &mut Ui) {
        if self.cfg.servers.is_empty() {
            return empty(ui, "Keine Server eingetragen.");
        }
        let instances = self.instances.clone();
        for (i, srv) in self.cfg.servers.clone().iter().enumerate() {
            let status = self.servers.get(i).cloned().flatten();
            let favicon = status
                .as_ref()
                .and_then(|s| s.favicon.as_deref())
                .and_then(|f| self.data_image(ui.ctx(), &format!("favicon:{}:{}", srv.address, f.len()), f));
            let inst = srv.instance.as_ref().and_then(|id| instances.iter().find(|i| &i.id == id));
            panel(ui, |ui| {
                ui.horizontal(|ui| {
                    block_icon(ui, favicon, &srv.name, 44.0);
                    ui.vertical(|ui| {
                        ui.set_width((ui.available_width() - 110.0).max(80.0));
                        ui.horizontal(|ui| {
                            theme::dot(ui, status.as_ref().is_some_and(|s| s.online));
                            title(ui, &srv.name, 13.0, theme::WHITE);
                        });
                        match &status {
                            None => {
                                muted(ui, "Pinge …");
                            }
                            Some(s) if s.online => {
                                ui.label(motd_job(&s.motd, ui.available_width()));
                            }
                            Some(s) => {
                                ui.label(RichText::new(format!("Offline · {}", s.error.as_deref().unwrap_or(""))).color(theme::RED));
                            }
                        }
                    });
                    ui.with_layout(Layout::top_down(Align::RIGHT), |ui| {
                        if let Some(s) = status.as_ref().filter(|s| s.online) {
                            ui.label(
                                RichText::new(format!("{} / {}", fmt_count(s.players.online), fmt_count(s.players.max)))
                                    .font(theme::mc_font(12.0))
                                    .color(theme::WHITE),
                            );
                            ui.label(RichText::new(format!("{} ms", s.latency)).font(theme::mc_font(10.0)).color(ping_color(s.latency)));
                        }
                        if let Some(inst) = inst {
                            let busy = self.launching.contains(&inst.id);
                            if ui.add(McButton::new("Beitreten").enabled(!busy)).clicked() {
                                self.launch(inst, Some(srv.address.clone()), format!("Verbinde mit {} …", srv.name));
                            }
                        }
                    });
                });
            });
        }
    }

    fn ui_recent(&mut self, ui: &mut Ui) {
        let instances = self.instances.clone();
        let recent: Vec<&Instance> = instances.iter().filter(|i| i.last_launch > 0).take(4).collect();
        if recent.is_empty() {
            return empty(ui, "Noch nichts gespielt.");
        }
        for inst in recent {
            let icon = inst.icon.as_deref().and_then(|u| self.data_image(ui.ctx(), &format!("inst:{}:{}", inst.id, u.len()), u));
            panel(ui, |ui| {
                ui.horizontal(|ui| {
                    block_icon(ui, icon, &inst.name, 36.0);
                    ui.vertical(|ui| {
                        ui.set_width((ui.available_width() - 50.0).max(60.0));
                        ui.add(egui::Label::new(RichText::new(&inst.name).font(theme::mc_font(13.0))).truncate());
                        muted(ui, format!("{} · {}", inst.mc_version, fmt_ago(inst.last_launch)));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let busy = self.launching.contains(&inst.id);
                        if ui.add(McButton::new("").icon(Icon::Play).kind(Kind::Play).enabled(!busy).tooltip("Spielen")).clicked() {
                            self.play_instance(inst);
                        }
                    });
                });
            });
        }
    }

    // ----- Instanzen -----

    fn ui_instances(&mut self, ui: &mut Ui) {
        let mut new_in_prism = false;
        view_head(ui, "Instanzen", "Direkt aus Prism Launcher – Änderungen in Prism erscheinen hier sofort.", |ui| {
            new_in_prism = ui.add(McButton::new("Neu in Prism").icon(Icon::Plus)).clicked();
            ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Suchen …").desired_width(200.0).margin(Margin::symmetric(8, 7)));
        });
        if new_in_prism {
            let r = backend::show_in_prism(None);
            self.report(r);
        }
        if !self.prism.as_ref().is_some_and(|p| p.found) {
            return empty(ui, "Prism Launcher nicht gefunden – siehe Einstellungen.");
        }
        let query = self.search.trim().to_lowercase();
        let instances = self.instances.clone();
        let list: Vec<&Instance> = instances
            .iter()
            .filter(|i| {
                query.is_empty()
                    || [Some(i.name.clone()), Some(i.mc_version.clone()), i.group.clone(), Some(loader_text(i))]
                        .into_iter()
                        .flatten()
                        .any(|v| v.to_lowercase().contains(&query))
            })
            .collect();
        if list.is_empty() {
            return empty(ui, if query.is_empty() { "Keine Instanzen in Prism." } else { "Keine Treffer." });
        }

        let gap = 10.0;
        let avail = ui.available_width();
        let cols = ((avail + gap) / (330.0 + gap)).floor().max(1.0) as usize;
        let card_w = (avail - gap * (cols - 1) as f32) / cols as f32;
        for row in list.chunks(cols) {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for inst in row {
                    ui.allocate_ui_with_layout(Vec2::new(card_w, 0.0), Layout::top_down(Align::Min), |ui| {
                        ui.set_width(card_w);
                        self.ui_instance_card(ui, inst);
                    });
                }
            });
        }
    }

    fn ui_instance_card(&mut self, ui: &mut Ui, inst: &Instance) {
        let icon = inst.icon.as_deref().and_then(|u| self.data_image(ui.ctx(), &format!("inst:{}:{}", inst.id, u.len()), u));
        panel(ui, |ui| {
            ui.horizontal_top(|ui| {
                block_icon(ui, icon, &inst.name, 48.0);
                ui.vertical(|ui| {
                    ui.add(egui::Label::new(RichText::new(&inst.name).font(theme::mc_font(14.0)).color(theme::WHITE)).truncate())
                        .on_hover_text(&inst.name);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
                        badge(ui, &inst.mc_version, theme::GREEN);
                        badge(ui, &loader_text(inst), theme::GRAY);
                        if inst.managed_by.is_some() {
                            badge(ui, "Modpack", theme::GOLD);
                        }
                        if let Some(g) = &inst.group {
                            badge(ui, g, theme::GRAY);
                        }
                    });
                });
            });
            ui.horizontal(|ui| {
                muted(ui, fmt_playtime(inst.total_played));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    muted(ui, fmt_ago(inst.last_launch));
                });
            });
            ui.horizontal(|ui| {
                let busy = self.launching.contains(&inst.id) || self.pack_for_instance(inst).is_some_and(|p| self.busy.contains(&p.source));
                let label = if busy { "Startet …" } else { "Spielen" };
                if ui.add(McButton::play(label).enabled(!busy)).clicked() {
                    self.play_instance(inst);
                }
                if ui.add(McButton::new("").icon(Icon::External).tooltip("In Prism bearbeiten (Mods, Einstellungen)")).clicked() {
                    let r = backend::show_in_prism(Some(&inst.id));
                    self.report(r);
                }
                if ui.add(McButton::new("").icon(Icon::Folder).tooltip("Ordner öffnen")).clicked() {
                    let r = backend::open_folder(Some(&inst.id));
                    self.report(r);
                }
            });
        });
    }

    // ----- Modpacks -----

    fn ui_packs(&mut self, ui: &mut Ui) {
        let mut refresh = false;
        view_head(ui, "Modpacks", "Werden als Prism-Instanz installiert und vor jedem Start automatisch aktualisiert.", |ui| {
            refresh = ui.add(McButton::new("Neu prüfen").icon(Icon::Refresh)).clicked();
        });
        if refresh {
            self.load_packs();
        }
        let packs = self.packs.clone();
        if packs.is_empty() {
            return empty(
                ui,
                "Noch keine Modpacks. Lege ein Modpack in der Admin-Oberfläche des Launcher-Servers an (/admin/) und veröffentliche es – es erscheint dann automatisch hier.",
            );
        }
        for pack in packs.iter() {
            if let Some(e) = &pack.error {
                panel_with(ui, theme::RED, |ui| {
                    title(ui, "Modpack nicht erreichbar", 14.0, theme::RED);
                    muted(ui, &pack.source);
                    ui.label(RichText::new(e).color(theme::RED));
                });
                continue;
            }
            let busy = self.busy.contains(&pack.source);
            let installed = pack.installed_version.is_some();
            panel(ui, |ui| {
                ui.horizontal_top(|ui| {
                    block_icon(ui, None, &pack.name, 56.0);
                    ui.vertical(|ui| {
                        title(ui, &pack.name, 16.0, theme::WHITE);
                        if !pack.description.is_empty() {
                            ui.label(&pack.description);
                        }
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
                            badge(ui, &pack.minecraft, theme::GREEN);
                            badge(ui, &pack.loader, theme::GRAY);
                            badge(ui, &format!("{} Dateien", pack.file_count), theme::GRAY);
                            match &pack.installed_version {
                                Some(_) if pack.up_to_date => badge(ui, &format!("v{}", pack.version), theme::GREEN),
                                Some(v) => badge(ui, &format!("Update {v} → {}", pack.version), theme::GOLD),
                                None => badge(ui, &format!("v{} · nicht installiert", pack.version), theme::GOLD),
                            };
                        });
                        ui.add_space(4.0);
                        if busy {
                            let p = self.progress.get(&pack.source);
                            theme::xp_bar(ui, progress_fraction(p), &progress_text(p));
                        } else {
                            ui.horizontal(|ui| {
                                if ui.add(McButton::play("Spielen")).clicked() {
                                    self.play_pack(pack, false);
                                }
                                let (icon, label) = match (installed, pack.up_to_date) {
                                    (false, _) => (Icon::Download, "Installieren"),
                                    (true, true) => (Icon::Refresh, "Prüfen"),
                                    (true, false) => (Icon::Refresh, "Aktualisieren"),
                                };
                                if ui
                                    .add(McButton::new(label).icon(icon).tooltip("Dateien mit dem Modpack vergleichen – zeigt erst, was sich ändert"))
                                    .clicked()
                                {
                                    self.review_pack(pack);
                                }
                            });
                        }
                    });
                });
            });
        }
    }

    fn ui_plan_modal(&mut self, ctx: &egui::Context) {
        let Some((pack, plan)) = self.plan.clone() else { return };
        let mut close = false;
        let mut apply = false;
        let frame = egui::Frame::new()
            .fill(Color32::from_rgb(0x16, 0x16, 0x18))
            .stroke(Stroke::new(theme::PX, Color32::BLACK))
            .inner_margin(Margin::same(18));
        let resp = egui::Modal::new(Id::new("plan"))
            .frame(frame)
            .backdrop_color(Color32::from_black_alpha(170))
            .show(ctx, |ui| {
                ui.set_width(620.0_f32.min(ctx.content_rect().width() - 80.0));
                title(ui, &format!("Abgleich – {}", plan.name), 17.0, theme::WHITE);
                muted(ui, plan_subtitle(&plan));
                ui.add_space(6.0);
                egui::ScrollArea::vertical().max_height(ctx.content_rect().height() * 0.55).show(ui, |ui| {
                    let mut any = false;
                    for (kind, label, mark, color) in [
                        (ChangeKind::Add, "Neu", "+", theme::GREEN),
                        (ChangeKind::Update, "Aktualisiert", "~", theme::GOLD),
                        (ChangeKind::Repair, "Wiederhergestellt", "!", theme::AQUA),
                        (ChangeKind::Remove, "Entfernt", "−", theme::RED),
                    ] {
                        let list: Vec<_> = plan.changes.iter().filter(|c| c.kind == kind).collect();
                        if list.is_empty() {
                            continue;
                        }
                        any = true;
                        section(ui, &format!("{label} ({})", list.len()));
                        for c in list.iter().take(80) {
                            diff_row(ui, mark, color, &c.path, c.size);
                        }
                        if list.len() > 80 {
                            muted(ui, format!("… und {} weitere", list.len() - 80));
                        }
                    }
                    if !any {
                        ui.label(if plan.unchanged > 0 {
                            format!("Alles stimmt: {} Datei{} wie im Modpack.", plan.unchanged, if plan.unchanged == 1 { "" } else { "en" })
                        } else {
                            "Dieses Modpack enthält keine Dateien.".into()
                        });
                    }
                    if !plan.extras.is_empty() {
                        let more = if plan.extras_truncated { "+" } else { "" };
                        section(ui, &format!("Eigene Dateien ({}{more})", plan.extras.len()));
                        muted(ui, "Nicht aus dem Modpack – werden nie überschrieben oder gelöscht.");
                        for e in &plan.extras {
                            diff_row(ui, "·", theme::GRAY, &e.path, Some(e.size));
                        }
                    }
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if !plan.changes.is_empty() {
                        let label = if plan.installed_version.is_some() { "Jetzt übernehmen" } else { "Installieren" };
                        apply = ui.add(McButton::play(label).icon(Icon::Download)).clicked();
                    }
                    close = ui.add(McButton::new("Schließen")).clicked();
                });
            });
        if resp.should_close() {
            close = true;
        }
        if apply {
            self.plan = None;
            self.play_pack(&pack, true);
        } else if close {
            self.plan = None;
        }
    }

    // ----- Einstellungen -----

    fn ui_settings(&mut self, ui: &mut Ui) {
        view_head(ui, "Einstellungen", &format!("{} {VERSION}", self.cfg.brand.name), |_| {});
        let p = self.prism.clone();

        section(ui, "Prism Launcher");
        panel(ui, |ui| {
            if let Some(p) = &p {
                egui::Grid::new("prism-status").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                    let key = |ui: &mut Ui, k: &str| {
                        ui.label(RichText::new(k).font(theme::mc_font(11.0)).color(theme::GRAY));
                    };
                    key(ui, "Status");
                    ui.horizontal(|ui| {
                        theme::dot(ui, p.found);
                        ui.label(if p.found {
                            "Verbunden"
                        } else if !p.exe_found {
                            "Programm nicht gefunden"
                        } else {
                            "Datenordner nicht gefunden"
                        });
                    });
                    ui.end_row();
                    key(ui, "Programm");
                    ui.label(if p.exe.is_empty() { "—" } else { &p.exe });
                    ui.end_row();
                    key(ui, "Daten");
                    ui.label(format!("{}{}", p.data_dir.display(), if p.portable { " (portable)" } else { "" }));
                    ui.end_row();
                    key(ui, "Instanzen");
                    ui.label(format!("{} in {}", self.instances.len(), p.instances_dir.display()));
                    ui.end_row();
                    key(ui, "Konten");
                    let names: Vec<_> = self.accounts.iter().map(|a| a.name.as_str()).collect();
                    ui.label(if names.is_empty() { "—".into() } else { names.join(", ") });
                    ui.end_row();
                });
                ui.add_space(6.0);
            }
            let exe_hint = p
                .as_ref()
                .map(|p| if p.exe.is_empty() { "Nicht gefunden – bitte auswählen".to_string() } else { format!("Automatisch: {}", p.exe) })
                .unwrap_or_default();
            let data_hint = p.as_ref().map(|p| format!("Automatisch: {}", p.default_data_dir.display())).unwrap_or_default();

            field_label(ui, "Programm (prismlauncher.exe)");
            let (changed, pick) = path_field(ui, &mut self.form.prism_exe, &exe_hint, "Durchsuchen");
            if changed && self.form.prism_exe.trim() != self.settings.prism_exe {
                self.save(json!({ "prismExe": self.form.prism_exe.trim() }));
            }
            if pick {
                self.pick(true);
            }
            field_label(ui, "Datenordner");
            let (changed, pick) = path_field(ui, &mut self.form.prism_data, &data_hint, "Ordner");
            if changed && self.form.prism_data.trim() != self.settings.prism_data_dir {
                self.save(json!({ "prismDataDir": self.form.prism_data.trim() }));
            }
            if pick {
                self.pick(false);
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.add(McButton::new("Prism öffnen").icon(Icon::External)).clicked() {
                    let r = backend::show_in_prism(None);
                    self.report(r);
                }
                if ui.add(McButton::new("Datenordner öffnen").icon(Icon::Folder)).clicked() {
                    let r = backend::open_folder(None);
                    self.report(r);
                }
            });
        });

        section(ui, "Launcher");
        panel(ui, |ui| self.ui_update(ui));

        section(ui, "Spielstart");
        panel(ui, |ui| {
            field_label(ui, "Nach dem Start");
            let options = [("minimize", "Launcher minimieren"), ("keep", "Launcher offen lassen"), ("close", "Launcher schließen")];
            let current = options.iter().find(|o| o.0 == self.settings.after_launch).map(|o| o.1).unwrap_or(options[0].1);
            let mut choice = None;
            egui::ComboBox::from_id_salt("after-launch").width(260.0).selected_text(current).show_ui(ui, |ui| {
                for (value, label) in options {
                    if ui.selectable_label(self.settings.after_launch == value, label).clicked() {
                        choice = Some(value);
                    }
                }
            });
            if let Some(v) = choice {
                self.save(json!({ "afterLaunch": v }));
            }
            ui.add_space(4.0);
            let mut auto = self.settings.auto_update_packs;
            if theme::checkbox(ui, &mut auto, "Modpacks vor dem Start automatisch aktualisieren").changed() {
                self.save(json!({ "autoUpdatePacks": auto }));
            }
            let mut join = self.settings.join_server;
            if theme::checkbox(ui, &mut join, "Beim Start eines Modpacks direkt dessen Server betreten").changed() {
                self.save(json!({ "joinServer": join }));
            }
        });
    }

    fn ui_update(&mut self, ui: &mut Ui) {
        let version = format!("{} {VERSION}", self.cfg.brand.name);
        let u = self.update.clone();
        ui.horizontal_wrapped(|ui| {
            title(ui, &version, 13.0, theme::WHITE);
            match &u {
                _ if self.update_checking => {
                    muted(ui, "Suche …");
                }
                Some(u) if self.update_busy => {
                    badge(ui, &format!("Update auf {}", u.version), theme::GOLD);
                }
                Some(u) if u.configured => {
                    if u.error.is_some() {
                        badge(ui, "Prüfung fehlgeschlagen", theme::RED);
                    } else if u.available {
                        badge(ui, &format!("Update auf {}", u.version), theme::GOLD);
                        if !u.pub_date.is_empty() {
                            badge(ui, &u.pub_date, theme::GRAY);
                        }
                        if u.size > 0 {
                            badge(ui, &fmt_bytes(u.size), theme::GRAY);
                        }
                    } else {
                        badge(ui, "Aktuell", theme::GREEN);
                    }
                }
                _ => {}
            }
        });
        if self.update_busy {
            let p = self.update_progress.as_ref();
            let (fraction, text) = match p {
                Some(p) if p.phase == "verify" => (1.0, "Prüfe Prüfsumme und Signatur …".to_string()),
                Some(p) if p.total_bytes > 0 => (p.bytes as f32 / p.total_bytes as f32, format!("Lade {} / {}", fmt_bytes(p.bytes), fmt_bytes(p.total_bytes))),
                Some(p) => (0.0, format!("Lade {}", fmt_bytes(p.bytes))),
                None => (0.0, "Starte …".into()),
            };
            theme::xp_bar(ui, fraction, &text);
        } else {
            if let Some(u) = &u {
                if !u.configured {
                    muted(ui, "Für diesen Launcher ist kein Update-Server hinterlegt.");
                }
                if let Some(e) = &u.error {
                    ui.label(RichText::new(e).color(theme::RED));
                }
                if u.available && !u.notes.is_empty() {
                    ui.label(&u.notes);
                }
            }
            ui.horizontal(|ui| {
                if ui.add(McButton::new("Nach Updates suchen").icon(Icon::Refresh).enabled(!self.update_checking)).clicked() {
                    self.check_update(false);
                }
                if let Some(u) = u.as_ref().filter(|u| u.available) {
                    if ui.add(McButton::play(&format!("Auf {} aktualisieren", u.version)).icon(Icon::Download)).clicked() {
                        self.install_update();
                    }
                }
            });
        }
        ui.add_space(4.0);
        let mut auto = self.settings.auto_update_launcher;
        if theme::checkbox(ui, &mut auto, "Automatisch nach Launcher-Updates suchen").changed() {
            self.save(json!({ "autoUpdateLauncher": auto }));
        }
    }

    // ----- Overlays -----

    fn ui_toasts(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        self.toasts.retain(|t| t.until > now);
        if self.toasts.is_empty() {
            return;
        }
        // Wie die Fortschritts-Einblendungen im Spiel: oben rechts, dunkle Fläche mit Fase.
        egui::Area::new(Id::new("toasts"))
            .anchor(Align2::RIGHT_TOP, Vec2::new(-16.0, TITLE_H + 12.0))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                ui.set_width(340.0);
                for t in &self.toasts {
                    let (head, color, icon) = match t.tone {
                        Tone::Ok => ("Erledigt!", theme::YELLOW, Icon::Chest),
                        Tone::Error => ("Fehler", theme::RED, Icon::Close),
                    };
                    let bg = ui.painter().add(egui::Shape::Noop);
                    let inner = egui::Frame::new().inner_margin(Margin { left: 50, right: 14, top: 10, bottom: 12 }).show(ui, |ui| {
                        ui.set_width(340.0 - 64.0);
                        ui.label(RichText::new(head).font(theme::mc_font(12.0)).color(color));
                        ui.label(RichText::new(&t.text).color(theme::WHITE));
                    });
                    let rect = inner.response.rect;
                    let mut shapes = theme::bevel_shapes(rect, Color32::from_rgb(0x21, 0x21, 0x24), Color32::from_rgb(0x55, 0x55, 0x5A), Color32::from_rgb(0x10, 0x10, 0x10), true);
                    shapes.push(egui::Shape::rect_filled(
                        Rect::from_min_size(rect.min + Vec2::new(12.0, 12.0), Vec2::splat(26.0)),
                        CornerRadius::ZERO,
                        Color32::from_black_alpha(160),
                    ));
                    ui.painter().set(bg, egui::Shape::Vec(shapes));
                    icon.paint(ui.painter(), rect.min + Vec2::new(17.0, 17.0), 2.0, color);
                    ui.add_space(6.0);
                }
            });
        let next = self.toasts.iter().map(|t| t.until).min().unwrap_or(now);
        ctx.request_repaint_after(next.saturating_duration_since(now));
    }

    /// Ohne Systemrahmen: Größe an den Fensterkanten selbst ändern.
    fn resize_edges(&self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
            return;
        }
        let r = ctx.content_rect();
        let e = 5.0;
        let zones = [
            (Rect::from_min_max(r.min, Pos2::new(r.min.x + e * 2.0, r.min.y + e * 2.0)), ResizeDirection::NorthWest, egui::CursorIcon::ResizeNwSe),
            (Rect::from_min_max(Pos2::new(r.max.x - e * 2.0, r.max.y - e * 2.0), r.max), ResizeDirection::SouthEast, egui::CursorIcon::ResizeNwSe),
            (Rect::from_min_max(Pos2::new(r.max.x - e * 2.0, r.min.y), Pos2::new(r.max.x, r.min.y + e * 2.0)), ResizeDirection::NorthEast, egui::CursorIcon::ResizeNeSw),
            (Rect::from_min_max(Pos2::new(r.min.x, r.max.y - e * 2.0), Pos2::new(r.min.x + e * 2.0, r.max.y)), ResizeDirection::SouthWest, egui::CursorIcon::ResizeNeSw),
            (Rect::from_min_max(r.min, Pos2::new(r.max.x, r.min.y + e)), ResizeDirection::North, egui::CursorIcon::ResizeVertical),
            (Rect::from_min_max(Pos2::new(r.min.x, r.max.y - e), r.max), ResizeDirection::South, egui::CursorIcon::ResizeVertical),
            (Rect::from_min_max(r.min, Pos2::new(r.min.x + e, r.max.y)), ResizeDirection::West, egui::CursorIcon::ResizeHorizontal),
            (Rect::from_min_max(Pos2::new(r.max.x - e, r.min.y), r.max), ResizeDirection::East, egui::CursorIcon::ResizeHorizontal),
        ];
        for (i, (rect, dir, cursor)) in zones.into_iter().enumerate() {
            let resp = ui.interact(rect, Id::new(("resize", i)), Sense::drag());
            if resp.hovered() || resp.dragged() {
                ctx.set_cursor_icon(cursor);
            }
            if resp.drag_started() {
                ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
            }
        }
    }
}

fn count(n: usize) -> String {
    if n == 0 { String::new() } else { n.to_string() }
}

fn ping_color(ms: u64) -> Color32 {
    match ms {
        0..150 => theme::GREEN,
        150..300 => theme::YELLOW,
        300..600 => theme::GOLD,
        _ => theme::RED,
    }
}

fn plan_subtitle(plan: &PackPlan) -> String {
    let mut parts = vec![match &plan.installed_version {
        None => format!("v{} · noch nicht installiert", plan.version),
        Some(v) if v == &plan.version => format!("Version {}", plan.version),
        Some(v) => format!("{v} → {}", plan.version),
    }];
    if !plan.changes.is_empty() {
        let n = plan.changes.len();
        parts.push(format!("{n} Änderung{}", if n == 1 { "" } else { "en" }));
    }
    if plan.download_bytes > 0 {
        parts.push(format!("{} zu laden", fmt_bytes(plan.download_bytes)));
    }
    parts.push(format!("{} unverändert", plan.unchanged));
    parts.join(" · ")
}

fn diff_row(ui: &mut Ui, mark: &str, color: Color32, path: &str, size: Option<u64>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(mark).font(theme::mc_font(12.0)).color(color));
        let w = ui.available_width() - 70.0;
        ui.add_sized([w.max(50.0), 16.0], egui::Label::new(RichText::new(path).monospace()).truncate())
            .on_hover_text(path);
        if let Some(s) = size {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| muted(ui, fmt_bytes(s)));
        }
    });
}

fn field_label(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(text).font(theme::mc_font(11.0)).color(theme::GRAY));
}

fn path_field(ui: &mut Ui, value: &mut String, hint: &str, button: &str) -> (bool, bool) {
    ui.horizontal(|ui| {
        let resp = ui.add(
            egui::TextEdit::singleline(value)
                .hint_text(hint)
                .desired_width((ui.available_width() - 140.0).clamp(160.0, 520.0))
                .margin(Margin::symmetric(8, 7)),
        );
        let pick = ui.add(McButton::new(button).icon(Icon::Folder)).clicked();
        (resp.lost_focus(), pick)
    })
    .inner
}

// ---------- eframe ----------

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.rx.try_recv() {
            self.handle(ctx, msg);
        }
        if let Some(t) = self.instances_dirty {
            if t.elapsed() >= Duration::from_millis(400) {
                self.instances_dirty = None;
                self.load_instances();
            } else {
                ctx.request_repaint_after(Duration::from_millis(400));
            }
        }
        // Beim Zurückkehren ins Fenster Prism neu lesen und ggf. nach Updates sehen
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(false));
        if focused && !self.was_focused {
            self.refresh_prism();
            if self.last_check.elapsed() >= FOCUS_CHECK_MIN {
                self.background_check();
            }
        }
        self.was_focused = focused;
        if self.last_check.elapsed() >= CHECK_INTERVAL {
            self.background_check();
        }
        if self.last_ping.elapsed() >= PING_INTERVAL {
            self.ping_servers();
        }
        ctx.request_repaint_after(Duration::from_secs(5));
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let full = ui.max_rect();
        theme::tile(ui.painter(), full, &self.bg, 4.0, Color32::WHITE);

        egui::Panel::top("titlebar")
            .exact_size(TITLE_H)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(Color32::from_rgb(0x0C, 0x0C, 0x0E)))
            .show(ui, |ui| self.ui_titlebar(ui));

        egui::Panel::left("sidebar")
            .exact_size(SIDEBAR_W)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(Color32::from_black_alpha(170)).inner_margin(Margin::same(12)))
            .show(ui, |ui| {
                let r = ui.max_rect().expand(12.0);
                ui.painter().rect_filled(
                    Rect::from_min_max(Pos2::new(r.max.x - theme::PX, r.min.y), r.max),
                    CornerRadius::ZERO,
                    Color32::BLACK,
                );
                self.ui_sidebar(ui);
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::from_black_alpha(80)).inner_margin(Margin { left: 24, right: 8, top: 20, bottom: 0 }))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    ui.set_max_width(ui.available_width() - 16.0);
                    match self.view {
                        View::Home => self.ui_home(ui),
                        View::Instances => self.ui_instances(ui),
                        View::Packs => self.ui_packs(ui),
                        View::Settings => self.ui_settings(ui),
                    }
                    ui.add_space(24.0);
                });
            });

        let ctx = ui.ctx().clone();
        self.ui_plan_modal(&ctx);
        self.ui_toasts(&ctx);
        self.resize_edges(ui);
        // Esc schließt den Dialog (Modal erledigt das selbst), Rahmen um das ganze Fenster
        ui.painter().rect_stroke(full, CornerRadius::ZERO, Stroke::new(1.0, Color32::BLACK), StrokeKind::Inside);
    }
}
