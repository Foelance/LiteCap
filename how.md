# LiteCap'i Nasıl Oluşturdum?

Bu dosyada, LiteCap'i nasıl yaptığımı anlatıyorum. Amacım: bu repoyu ileride ben ya da başka biri tekrar açtığında "bu nereden geldi, nasıl bu hale geldi" sorusuna cevap bulabilmesi.

Projeyi baştan sona bir AI coding ajanına yazdırdım. Aşağıdaki 1. bölüm, benim başka bir oturumda ajana yaptırdığım ilk yazım; 2. bölüm ise sonradan aynı ajana "bunu GitHub'a yükle" dediğimde yaptırdıklarım. İlk oturumun konuşma kaydı elimde değil, o yüzden 1. bölümü ajanın sonradan git geçmişine ve koda bakarak çıkardığı bilgilerden derledim — kesin bir "şu şekilde düşünüldü" anlatısı değil, koddan çıkarım.

---

## 1. Çekirdek uygulamayı yazdırdım

Git geçmişi 3 commit'ten oluşuyor, hepsi art arda ~40 dakika içinde:

1. **`7f2b066` — "LiteCap: low-RAM cross-platform screen recorder"**
   Tek seferde ~6600 satırlık ilk sürüm: `Cargo.toml`, `main.rs`, `recorder.rs`, `ffmpeg.rs`, `config.rs`, `icon.rs`, platform-özel capture modülleri (`capture/win.rs`, `capture/wayland.rs`, `capture/x11.rs`), Windows ses modülü (`audio_win.rs`, `audio_net.rs`) ve CI workflow'u (`ci.yml`) bir arada geldi.

2. **`dfc63eb` — "Add 1920x1080 @ 60fps recording preset"**
   `config.rs`, `main.rs`, `recorder.rs` üzerinde küçük bir özellik eklemesi: sabit 1080p60 çıktı zorlayan bir preset (kaynak monitörün gerçek çözünürlüğü/hızı ne olursa olsun).

3. **`4473a7e` — "Fix silent-audio race by starting capture before ffmpeg connects; add Options submenu"**
   Bir bug fix + küçük UI eklemesi: ses yakalamanın ffmpeg'e bağlanmadan *önce* başlatılması gerektiği bir race condition düzeltildi (`audio_win.rs`, `recorder.rs`), tepsi menüsüne bir "Options" alt menüsü eklendi (`main.rs`).

### Mimari

- **Dil/araç zinciri:** Rust (2021 edition), `cargo`.
- **Platform ayrımı derleme zamanında (`#[cfg(...)]`) yapılıyor** — tek kod tabanı, iki gerçek backend:
  - **Windows:** `windows-capture` crate'i ile Windows Graphics Capture API (in-process, düşük RAM); ses için `cpal` + özel `audio_win.rs`/`audio_net.rs`; tray/pencere için `tao` + `tray-icon`.
  - **Linux:** İki ayrı yakalama yolu —
    - Wayland: `ashpd` (XDG Desktop Portal `org.freedesktop.portal.ScreenCast`) ile kullanıcıdan izin alıp `pipewire` crate'i üzerinden frame okuma (`capture/wayland.rs`).
    - X11: in-process yakalama yok; ffmpeg'in kendi `x11grab`'ı kullanılıyor, monitör geometrisi `xrandr` çıktısından parse ediliyor (`capture/x11.rs`).
  - Tray/GUI için Linux'ta `gtk` (3.x) kullanılıyor.
- **Ortak katman:** `recorder.rs` bir kayıt oturumunu (ffmpeg alt süreci + video kaynağı + ses akışları + "pacer" thread'i) yönetiyor. `ffmpeg.rs`, ffmpeg binary'sini bulmuyorsa GitHub'daki `BtbN/FFmpeg-Builds` release'inden platforma özel bir build indirip `<data_dir>/ffmpeg/` altına önbelleğe alıyor — elle ffmpeg kurmama gerek kalmıyor.
- **Frame akışı RAM'de biriktirilmiyor:** `capture/mod.rs` içindeki `FrameSlot`/pacer tasarımı, en fazla ~2 frame buffer'ı canlı tutup en son kareyi sabit FPS'te ffmpeg'in stdin'ine yazıyor — README'deki "low-RAM" iddiasının kaynağı bu.
- **Config:** `directories` crate'i ile platforma uygun config dizininde TOML dosyası (`config.rs`).

Windows'ta bizzat kendim denedim, çalıştı. Ama şu ana kadar sadece ben denedim, başka test eden olmadı.

---

## 2. Ajana GitHub'a yükletip CI/CD kurdurdum, Linux tarafını düzelttirdim

Proje `C:\Users\efedo\Projects\litecap` altında yerel bir git reposuydu, hiçbir GitHub remote'u yoktu. Ajana "GitHub hesabıma yükle" dedim, o da adım adım şunları yaptı:

1. **Projeyi buldu:** Dosya sistemini tarayıp `Projects/litecap` klasörünü ve içindeki mevcut git geçmişini tespit etti.
2. **GitHub reposu oluşturdu:** GitHub REST API'sini kullanarak `Foelance/LiteCap` adında public bir repo açtı, `origin` remote'unu ekleyip yerel `master` dalını uzak `main`'e push etti.
3. **README yazdı:** Kaynak kodu (`Cargo.toml`, `main.rs`, `config.rs`, `ci.yml`) okuyup özellikleri, build adımlarını, config konumunu ve CI'ı özetleyen bir `README.md` hazırladı.
4. **Release pipeline'ı kurdu:** `v*` tag push'unda Windows + Linux'ta `cargo build --release` çalıştırıp iki binary'yi GitHub Releases'e yayınlayan `.github/workflows/release.yml` dosyasını ekledi.
5. **Linux build'in gerçekte hiç derlenmediğini ortaya çıkardı ve düzeltti.** İlk `v0.1.0` tag'ini push ettiğimizde CI'daki Linux job başarısız oldu — bunlar benim eklettiğim workflow'dan bağımsız, önceden var olan gerçek derleme hatalarıydı (yani Linux tarafını hiç test etmemişim, ilk elden öğrendim):
   - `ashpd = "0.13.12"` bağımlılığında `screencast` feature'ı açık değildi → `Screencast`/`PersistMode`/`SourceType` import'ları derlenmiyordu. **Düzeltme:** `features = ["screencast"]` eklendi.
   - `wayland.rs` doğrudan `tokio::runtime::Builder` kullanıyordu ama `tokio` crate'i `Cargo.toml`'da tanımlı değildi. **Düzeltme:** `tokio` bağımlılığı `rt`/`net`/`time`/`io-util` feature'larıyla eklendi.
   - `main.rs`, var olmayan bağımsız bir `glib` crate'ine referans veriyordu. **Düzeltme:** `gtk` crate'inin zaten re-export ettiği `gtk::glib` kullanıldı.
   - `ashpd::desktop::request::ResponseError` private bir modül içinde olduğu için dışarıdan isimlendirilemiyordu (iptal algılama kodu derlenmiyordu). **Düzeltme:** iptal tespiti ashpd'nin sabit `Display` metnine (`"Portal request was cancelled"`) bakacak şekilde yeniden yazıldı.
   - `.set_sources(SourceType::Monitor)` çağrısı `BitFlags<SourceType>` beklerken çıplak `SourceType` veriyordu (tip uyuşmazlığı). **Düzeltme:** `enumflags2::BitFlags::from(...)` ile sarmalandı.
   - Release job'ı GitHub'a yayın oluşturamıyordu çünkü varsayılan `GITHUB_TOKEN` izni salt-okunurdu. **Düzeltme:** workflow'a `permissions: contents: write` eklendi.

   Ajan her düzeltmeden sonra gerçek GitHub Actions çalıştırmalarını izleyip hata loglarını okudu, kök nedeni bulup düzeltti, tag'i taşıyıp yeniden tetikledi — toplam 5 deneme sonunda hem Windows hem Linux binary'si başarıyla derlenip [Releases](https://github.com/Foelance/LiteCap/releases/tag/v0.1.0) sayfasına yayınlandı.
6. **README'yi genişletti:** İndirme talimatları, Linux'a özel kurulum/çalıştırma rehberi (runtime kütüphaneleri, tray icon gereksinimleri, Wayland portal backend'leri vs. X11/`xrandr` ayrımı) ve projenin tamamen yapay zeka ile üretildiğini belirten bir uyarı notu eklendi.

### Ajanın bu adımlarda kullandığı araçlar

- Dosya sistemi tarama/okuma (proje konumunu bulmak, kaynak kodu okumak)
- `git` (commit, tag, remote, push)
- GitHub REST API (`curl` ile: repo oluşturma, About açıklaması güncelleme, Actions run/job/log sorgulama)
- Rust/`cargo` ekosistemi bilgisi + yerel `~/.cargo/registry` içindeki bağımlılık kaynak kodunu doğrudan okuyarak (`ashpd`, `gtk` crate'lerinin gerçek API'lerini teyit ederek) hataları düzeltme
- Web araması (ashpd API'sinin doğru modül yolunu doğrulamak için)

---

## Şu an bildiğim sınırlamalar

- Bu dosyanın 1. bölümü, çekirdek uygulamanın *nasıl tasarlandığına* dair kesin bir "düşünce süreci" kaydı değil — o oturumun konuşma geçmişi elimde yok. Mimari açıklaması ajanın koddan ve commit mesajlarından çıkardığı bilgilere dayanıyor.
- Windows tarafını bizzat denedim ve çalıştığını doğruladım — ama şimdiye kadar sadece ben test ettim, başka kimse denemedi.
- Linux tarafı hiç test edilmemişti: en az 6 farklı derleme/yayın hatası vardı ve bunlar ancak ajan gerçek CI çalıştırmalarını izleyerek bulup düzeltti — yani Linux desteğinin ilk hâli hiç derlenmeden commit edilmiş. Şu an derleniyor ve Releases'te binary var, ama kendim bir Linux makinede henüz denemedim.
- Ciddi bir insan code review'undan geçmedi. Bir şey bozuksa ya da yanlış görünüyorsa lütfen issue aç ya da PR gönder, her türlü öneriye açığım.
