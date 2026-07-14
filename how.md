# LiteCap Nasıl Oluşturuldu?

Bu doküman, LiteCap'in nasıl bir yapay zeka ajanı (AI coding agent) tarafından geliştirildiğini, hangi araçlarla ve hangi adımlarla ilerlendiğini anlatır. Amaç: bu repoyu ay sonra/yıl sonra açan birinin "bu nereden geldi, nasıl bu hale geldi" sorusuna cevap bulabilmesi.

> Not (dürüstlük payı): Aşağıdaki "1. Çekirdek Uygulamanın Yazılması" bölümü, bu dosyayı yazan oturumdan **önceki, ayrı bir AI oturumunda** yapıldı — ben (bu notu yazan ajan) o oturumun canlı tanığı değilim, elimde o oturumun konuşma kaydı yok. O kısmı, repodaki git geçmişinden ve kaynak kodun kendisinden **çıkarım yaparak** anlatıyorum, uydurmuyorum. "2. GitHub'a Yükleme, CI/CD ve Linux Düzeltmeleri" bölümü ise doğrudan benim bu oturumda yaptıklarım — adım adım, gerçek.

---

## 1. Çekirdek Uygulamanın Yazılması (önceki oturum, kod ve git geçmişinden çıkarım)

Git geçmişi 3 commit'ten oluşuyor, hepsi `LiteCap Builder <litecap@local>` yazarıyla, art arda ~40 dakika içinde:

1. **`7f2b066` — "LiteCap: low-RAM cross-platform screen recorder"**
   Tek seferde ~6600 satırlık ilk sürüm: `Cargo.toml`, `main.rs`, `recorder.rs`, `ffmpeg.rs`, `config.rs`, `icon.rs`, platform-özel capture modülleri (`capture/win.rs`, `capture/wayland.rs`, `capture/x11.rs`), Windows ses modülü (`audio_win.rs`, `audio_net.rs`) ve CI workflow'u (`ci.yml`) bir arada geldi. Commit'in tek parça ve büyük olması, ajanın uygulamayı planlayıp toplu halde ürettiğini gösteriyor (adım adım commit'lenmemiş).

2. **`dfc63eb` — "Add 1920x1080 @ 60fps recording preset"**
   `config.rs`, `main.rs`, `recorder.rs` üzerinde küçük bir özellik eklemesi: sabit 1080p60 çıktı zorlayan bir preset (kaynak monitörün gerçek çözünürlüğü/hızı ne olursa olsun).

3. **`4473a7e` — "Fix silent-audio race by starting capture before ffmpeg connects; add Options submenu"**
   Bir bug fix + küçük UI eklemesi: ses yakalamanın ffmpeg'e bağlanmadan *önce* başlatılması gerektiği bir race condition düzeltildi (`audio_win.rs`, `recorder.rs`), tepsi menüsüne bir "Options" alt menüsü eklendi (`main.rs`).

### Mimari (koddan doğrulanmış)

- **Dil/araç zinciri:** Rust (2021 edition), `cargo`.
- **Platform ayrımı derleme zamanında (`#[cfg(...)]`) yapılıyor** — tek kod tabanı, iki gerçek backend:
  - **Windows:** `windows-capture` crate'i ile Windows Graphics Capture API (in-process, düşük RAM); ses için `cpal` + özel `audio_win.rs`/`audio_net.rs`; tray/pencere için `tao` + `tray-icon`.
  - **Linux:** İki ayrı yakalama yolu —
    - Wayland: `ashpd` (XDG Desktop Portal `org.freedesktop.portal.ScreenCast`) ile kullanıcıdan izin alıp `pipewire` crate'i üzerinden frame okuma (`capture/wayland.rs`).
    - X11: in-process yakalama yok; ffmpeg'in kendi `x11grab`'ı kullanılıyor, monitör geometrisi `xrandr` çıktısından parse ediliyor (`capture/x11.rs`).
  - Tray/GUI için Linux'ta `gtk` (3.x) kullanılıyor.
- **Ortak katman:** `recorder.rs` bir kayıt oturumunu (ffmpeg alt süreci + video kaynağı + ses akışları + "pacer" thread'i) yönetiyor. `ffmpeg.rs`, ffmpeg binary'sini bulmuyorsa GitHub'daki `BtbN/FFmpeg-Builds` release'inden platforma özel bir build indirip `<data_dir>/ffmpeg/` altına önbelleğe alıyor — kullanıcı elle ffmpeg kurmak zorunda değil.
- **Frame akışı RAM'de biriktirilmiyor:** `capture/mod.rs` içindeki `FrameSlot`/pacer tasarımı, en fazla ~2 frame buffer'ı canlı tutup en son kareyi sabit FPS'te ffmpeg'in stdin'ine yazıyor — bu, README'deki "low-RAM" iddiasının kaynağı.
- **Config:** `directories` crate'i ile platforma uygun config dizininde TOML dosyası (`config.rs`).

---

## 2. GitHub'a Yükleme, CI/CD ve Linux Düzeltmeleri (bu oturum, doğrudan gerçekleştirdiğim adımlar)

Kullanıcı "Litecap'i GitHub'a yükle" dediğinde proje `C:\Users\efedo\Projects\litecap` altında yerel bir git reposuydu, hiçbir remote'u yoktu. Adım adım:

1. **Projeyi bulma:** Dosya sistemini tarayıp `Projects/litecap` klasörünü ve içindeki mevcut git geçmişini tespit ettim.
2. **GitHub reposu oluşturma:** GitHub REST API'sini (mevcut bir personal access token ile) kullanarak `Foelance/LiteCap` adında public bir repo oluşturdum, `origin` remote'unu ekleyip yerel `master` dalını uzak `main`'e push ettim.
3. **README yazımı:** Kaynak kodu (`Cargo.toml`, `main.rs`, `config.rs`, `ci.yml`) okuyup özellikleri, build adımlarını, config konumunu ve CI'ı özetleyen bir `README.md` yazdım.
4. **Release pipeline eklendi:** `v*` tag push'unda Windows + Linux'ta `cargo build --release` çalıştırıp iki binary'yi GitHub Releases'e yayınlayan `.github/workflows/release.yml` yazdım.
5. **Linux build'in gerçekte bozuk olduğunu keşfettim ve düzelttim.** İlk `v0.1.0` tag'ini push ettiğimde CI'daki Linux job hep başarısız oldu — bu, benim eklediğim workflow'dan bağımsız, önceden var olan gerçek derleme hatalarıydı:
   - `ashpd = "0.13.12"` bağımlılığında `screencast` feature'ı açık değildi → `Screencast`/`PersistMode`/`SourceType` import'ları derlenmiyordu. **Düzeltme:** `features = ["screencast"]` eklendi.
   - `wayland.rs` doğrudan `tokio::runtime::Builder` kullanıyordu ama `tokio` crate'i `Cargo.toml`'da tanımlı değildi. **Düzeltme:** `tokio` bağımlılığı `rt`/`net`/`time`/`io-util` feature'larıyla eklendi.
   - `main.rs`, var olmayan bağımsız bir `glib` crate'ine referans veriyordu. **Düzeltme:** `gtk` crate'inin zaten re-export ettiği `gtk::glib` kullanıldı.
   - `ashpd::desktop::request::ResponseError` private bir modül içinde olduğu için dışarıdan isimlendirilemiyordu (iptal algılama kodu derlenmiyordu). **Düzeltme:** iptal tespiti ashpd'nin sabit `Display` metnine (`"Portal request was cancelled"`) bakacak şekilde yeniden yazıldı.
   - `.set_sources(SourceType::Monitor)` çağrısı `BitFlags<SourceType>` beklerken çıplak `SourceType` veriyordu (tip uyuşmazlığı). **Düzeltme:** `enumflags2::BitFlags::from(...)` ile sarmalandı.
   - Release job'ı GitHub'a yayın oluşturamıyordu çünkü varsayılan `GITHUB_TOKEN` izni salt-okunurdu. **Düzeltme:** workflow'a `permissions: contents: write` eklendi.

   Her düzeltmeden sonra gerçek GitHub Actions çalıştırmalarını GitHub API üzerinden izleyip (`workflow runs` / `jobs` / `logs` endpoint'leri) hata loglarını okudum, kök nedeni bulup düzelttim ve tag'i taşıyıp yeniden tetikledim — toplam 5 deneme sonunda hem Windows hem Linux binary'si başarıyla derlenip [Releases](https://github.com/Foelance/LiteCap/releases/tag/v0.1.0) sayfasına yayınlandı.
6. **README genişletildi:** İndirme talimatları, Linux'a özel kurulum/çalıştırma rehberi (runtime kütüphaneleri, tray icon gereksinimleri, Wayland portal backend'leri vs. X11/`xrandr` ayrımı) ve projenin tamamen yapay zeka ile üretildiğini belirten bir uyarı notu eklendi.

### Kullanılan araçlar (bu oturumda)

- Dosya sistemi tarama/okuma (proje konumunu bulmak, kaynak kodu okumak)
- `git` (commit, tag, remote, push)
- GitHub REST API (`curl` ile: repo oluşturma, About açıklaması güncelleme, Actions run/job/log sorgulama)
- Rust/`cargo` ekosistemi bilgisi + yerel `~/.cargo/registry` içindeki bağımlılık kaynak kodunu doğrudan okuyarak (`ashpd`, `gtk` crate'lerinin gerçek API'lerini teyit ederek) hataları düzeltme
- Web araması (ashpd API'sinin doğru modül yolunu doğrulamak için)

---

## Bilinen sınırlamalar / dürüstçe belirtilmesi gerekenler

- Bu doküman, çekirdek uygulamanın *nasıl tasarlandığına* dair kesin bir "düşünce süreci" kaydı değildir — o oturumun konuşma geçmişi elimde yok. Yukarıdaki mimari açıklaması tamamen mevcut koddan ve commit mesajlarından çıkarılmıştır.
- Uygulama, README'de de belirtildiği gibi, ciddi bir insan code review'undan geçmemiştir. Windows tarafı proje sahibi tarafından bizzat denenmiş ve çalıştığı doğrulanmıştır — ancak şimdiye kadar sadece kendisi test etti, başka kimse denemedi. Linux tarafı ise hiç test edilmemişti: en az 6 farklı derleme/yayın hatası vardı ve bunlar ancak bu oturumda gerçek CI çalıştırmaları izlenerek bulunup düzeltildi — bu da Linux desteğinin ilk halinin hiç derlenmeden commit edildiğini gösteriyor.
