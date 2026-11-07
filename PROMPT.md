# Firewol File Explorer — Master Specification & Agent Prompt

> **Rol:** Kıdemli Sistem Mimarı, Rust/Sistem Mühendisi ve UI/UX Mühendisi.  
> **Görev:** Windows Dosya Gezgini'nin (Windows Explorer) hantal, yavaş ve kısıtlayıcı yapısına alternatif; ultra hafif (tek pencere ~150 MB RAM), anlık arama (instant search) yeteneğine sahip, aşırı derecede özelleştirilebilir ve açık kaynaklı yeni nesil bir masaüstü dosya yöneticisi (Firewol) geliştirmek.

---

## 1. Firewol Temel Amacı ve Vizyonu

* **Sıfırdan Yeni Bir File Explorer:** Windows'un varsayılan dosya gezginine bağımlı olmayan, sıfırdan inşa edilmiş bağımsız masaüstü uygulaması.
* **Windows Explorer Yavaşlığı ve Kısıtlamalarından Kurtuluş:**
  * Windows Explorer'ın 30 dakikaya varan, dizin taraması yaparken donan ve çok yavaş çalışan arama sistemine kesin alternatif oluşturmak.
  * Windows'un sunduğu özelleştirilemez, katı arayüz yapısını ortadan kaldırmak.
* **Gereksiz Kirlilikten ve Dayatmalardan Arınma (Sadelik):**
  * Kişiselleştirilemeyen "Home" sekmesi dayatmasından kurtulmak.
  * Kaldırılamayan gereksiz butonları ve araç çubuğu kirliliğini tamamen yok etmek.
  * Sürekli girilen klasörlerin kullanıcının izni olmadan hemen "Hızlı Erişim"e (Quick Access) yerleşmesi gibi istenmeyen davranışları engellemek.
* **Tasarım Felsefesi:**
  * **Hız ve Özelleştirilebilirlik:** Uygulama olabildiğince maksimum hız, anlık tepki süresi ve sınırsız özelleştirilebilirlik üzerine kuruludur.
  * **Minimalizm:** Uygulama ilk açıldığında varsayılan olarak minimal, göz yormayan, son derece sade ve temiz bir arayüzle gelmelidir.

---

## 2. Kesin Kurallar ve Katı Kısıtlamalar (Invariants)

1. **Bellek (RAM) Bütçesi:**
   * Tek pencere boşta **~150 MB** civarında kalmalı; en kötü durumda (4 pencere + indeksleme + transfer) **250 MB'ın altında** olmalıdır (2026-11-07'de 150 MB kesin sınırından gevşetildi).
   * `everything.exe` gibi sürekli arka planda açık kaldığında yüksek RAM tüketen araçların aksine, Firewol belleği şişirmeden anlık arama sunmalıdır.
2. **Sıfır Dosya Sistemi Kirliliği (Clean Filesystem):**
   * Kullanıcının yaptığı klasör rengi, simge ve etiket özelleştirmeleri dosya sistemine (`desktop.ini` vb.) dokunmaz. Tüm özelleştirmeler Firewol'un yerel hafif konfigürasyon/veritabanı katmanında izole tutulur.
3. **Açık Kaynak Kod (Open Source):**
   * Proje tamamen açık kaynaklıdır; kod tabanı modüler ve temiz olmalı, kullanıcı beğenmediği her mantığı veya bileşeni rahatça değiştirebilmelidir.
4. **Veri ve İşlem Güvenliği:**
   * Yanlışlıkla yapılan silme işlemlerine karşı Windows Geri Dönüşüm Kutusu (Recycle Bin) entegrasyonu ve Geri Al (`Ctrl+Z`) desteği esastır.

---

## 3. Teknoloji Yığını ve Sistem Mimarisi

* **Arka Plan (Backend / Core Engine):** **Rust (Tauri v2 Core)**
  * Düşük seviyeli bellek kontrolü (~40–80 MB taban tüketim).
  * Win32 API (`windows-rs`), NTFS MFT / USN Journal doğrudan okuma yeteneği.
  * Çok iş parçacıklı (multithreaded) paralel asenkron dosya motoru (`tokio`, `rayon`).
* **Ön Yüz (Frontend / UI Layer):** **Tauri v2 + Svelte (veya React) + TailwindCSS / Modern CSS Engine**
  * Webview2 yerel katmanı üzerinden sıfır Electron yükü.
  * Yüksek kare hızı (60–120 FPS), akıcı mikro etkileşimler ve animasyonlar.
* **Arama ve İndeksleme Katmanı:** **Rust Yerel NTFS MFT Parser + SQLite FTS5 (veya Tantivy Disk Index)**
  * RAM'de milyonlarca dosya nesnesi tutulmaz; disk üzerinde sıkıştırılmış B-Tree / FTS5 yapısı kullanılır.

---

## 4. Temel Özellikler ve Modül Detayları

### 4.1. Anlık Arama Motoru (Instant Search Subsystem)
* **Everything-Seviyesi Hız, Sıfır RAM Yükü:**
  * Windows Explorer gibi 30 dakika süren dosya taramalarına son; arama sonuçları milisaniyeler içinde (instant) listelenir.
  * NTFS disklerde Master File Table (MFT) ve USN Journal doğrudan okunur; ilk açılışta hızlı ve arka planda hafif bir disk önbelleği (SQLite FTS5 / Tantivy) oluşturulur.
  * Milyonlarca dosya RAM'e boca edilmez; bellek bütçesi aşılmadan anlık sorgulama disk önbelleğinden çekilir.
* **Arama Filtreleri:** Uzantı, dosya boyutu, değiştirilme tarihi ve anlık regex desteği.

### 4.2. Kapsamlı Özelleştirilebilirlik ve Tema Motoru (Styling & Theme Presets)
* **Geniş ve Yuvarlak Renk Paleti:**
  * Kullanıcı uygulamanın tüm tema renklerini geniş, dairesel/yuvarlak etkileşimli bir renk paleti (color wheel / palette) üzerinden seçer.
  * Palet üzerinden düzenlenebilir alanlar:
    * Varsayılan klasör rengi (Default Folder Color)
    * Varsayılan klasör adı rengi (Default Folder Label Color)
    * Varsayılan arka plan teması (Default Background Theme)
    * Varsayılan ikincil renk (Default Secondary / Accent Color)
    * Ve arayüzdeki diğer tüm vurgu tonları.
* **Klasör Özel Görünümleri:**
  * Kullanıcı istediği klasörün rengini, simgesini (icon) ve ismini uygulama içinde rahatlıkla değiştirebilir.
  * Bu değişiklikler dosya sistemine zarar vermeden Firewol yerel metadata katmanında saklanır.
* **Pencere Kontrolleri Kişiselleştirmesi (Window Controls):**
  * Kapatma, küçültme ve tam ekran butonlarının:
    * Yeri (sağ üst veya sol üst / macOS tarzı)
    * Şekli (yumuşak kenarlı, köşeli, dairesel, minimalist tuş stilleri)
    * Renkleri ve hover efektleri serbestçe değiştirilebilir.
* **7 Hazır Tasarım Preseti (Design Presets):**
  1. **Neubrutalism:** Kalın siyah kenarlıklar, yüksek kontrastlı canlı renkler, sert gölgeler.
  2. **Skeuomorphism:** Gerçek dünya dokuları, kabartmalar, derinlik ve dikiş detayları.
  3. **Neumorphism:** Yumuşak iç ve dış gölgeler, zeminle bütünleşen gömülü butonlar.
  4. **Glassmorphism:** Yarı saydam buzlu cam paneller (backdrop-blur), ince ışık sınırları.
  5. **Claymorphism:** 3D killi şişkin düğmeler, pürüzsüz yuvarlatılmış hacimler ve pastel tonlar.
  6. **Light-morphism:** Parlak, ışık odaklı minimalist yarı saydam katmanlar.
  7. **Metal-morphism:** Fırçalanmış alüminyum, krom yansımalar ve metalik dokulu arayüz.

### 4.3. Sağ Tık Menüsü (Nilesoft Shell Düzeni & Hibrit Entegrasyon)
* **Nilesoft Shell Şablonu:**
  * Sağ tık menüsü varsayılan olarak Windows için geliştirilmiş popüler **Nilesoft Shell** arayüz düzeni ve ergonomisiyle gelir.
* **Tamamen Düzenlenebilir Yapı:**
  * Kullanıcı sağ tık menüsünü dilediği gibi sürükle-bırak veya ayar menüsünden yeniden düzenleyebilir:
    * Menü öğelerinin yerini değiştirme
    * Renk ve simge değişikliği
    * İstenmeyen Windows varsayılan öğelerini çıkarma
    * Özel eylemler (Open in Terminal, VS Code, Hash Check vb.) ekleme.
* **Hibrit Entegrasyon Kapsamı:**
  * **Varsayılan:** Sadece Firewol içerisindeki dosya ve klasörlerde çalışır.
  * **İsteğe Bağlı (Opsiyonel):** Ayarlar menüsünden kullanıcının tek tıkla onayıyla Windows genel sağ tık menüsüne de (Explorer & Desktop) kaydedilebilir.

### 4.4. Gezinme, Sekmeler ve Çift Panel Mimarisi (Tabs & Split-Pane)
* **Sekmeli Gezinme (Tabs):** Tarayıcı benzeri çoklu sekme desteği, sekmeleri sürükleme ve ayırma.
* **İsteğe Bağlı Çift Panel (Split / Dual-Pane View):** İki farklı klasörü yan yana görüntüleme ve paneller arası hızlı sürükle-bırak ile kopyalama/taşıma.
* **Temizlenebilir Yan Panel (Customizable Sidebar):**
  * Windows Explorer'ın aksine sabit ve kilitli değildir.
  * İstenmeyen kısayollar tek tıkla gizlenebilir; kullanıcı sadece istediği klasörleri ekler.
  * Tamamen gizlenebilir (zen modu).
* **Akıllı Breadcrumb Yol Çubuğu:** Klasör hiyerarşisi arasında anında atlama ve düzenlenebilir metin yolu girişi.

### 4.5. Büyük Dizin Performansı ve Gelişmiş Paralel Transfer Motoru
* **Sanal Liste (Virtual Scrolling):** 100.000+ dosya içeren klasörlerde dahi yalnızca ekranda görünen öğeleri çizerek sıfır donma ve anlık kaydırma performansı.
* **Gelişmiş Paralel Dosya Transferi:**
  * Çoklu iş parçacığı (multithreading) ile büyük dosya ve çoklu klasör kopyalama/taşıma işlemlerini maksimum SSD/HDD bant genişliğinde hızlandırma.
  * Checksum (CRC32/SHA-256) doğrulama ile veri bozulmasını engelleme.
  * Detaylı, gerçek zamanlı transfer hızı grafiği, duraklatma ve devam ettirme kontrolleri.
  * Geri Al (`Ctrl+Z`) işlem geçmişi ve Windows Geri Dönüşüm Kutusu entegrasyonu.

### 4.6. Ayarlar Menüsü ve Güvenlik (Privilege & Permission Manager)
* **Kapsamlı Ayarlar Alanı:** Kullanıcının tüm uygulama ayarlarını, tema paletlerini ve varsayılan görünümlerini tek merkezden yönetebildiği alan.
* **Varsayılan İzin / Yönetici İzni Yönetimi (Privilege Escalation Manager):**
  * Firewol varsayılan olarak standart kullanıcı (non-elevated) modunda çalışır.
  * Ayarlar menüsünden:
    * Hangi uygulamaların veya eylemlerin otomatik olarak yönetici (Admin / UAC) izniyle çalıştırılacağı seçilebilir (Beyaz Liste).
    * Alternatif olarak kullanıcı dilerse tüm işlemlere doğrudan izin verebilir (**Her şeye izin ver — Yüksek Risk** modu açıkça risk uyarısıyla sunulur).
    * Arka planda UAC onaylı güvenli yardımcı worker mimarisi kullanılır.
* **Özelleştirme Ayarları:**
  * Yuvarlak renk paletleri ve tema motoru yapılandırmaları.
  * Varsayılan görünümler (ızgara boyutu, liste görünümü, ayrıntılar, sütun düzenleri).

---

## 5. Performans ve Kalite Metrikleri (SLAs)

| Metrik | Hedef Değer | Kısıtlama |
| :--- | :--- | :--- |
| **Maksimum RAM Tüketimi** | `< 250 MB` | En kötü durum: 4 pencere + indeksleme + transfer |
| **Boşta Çalışma (Idle) RAM** | `~150 MB` | Tek pencere |
| **Arama Yanıt Süresi** | `< 100 ms` | Anlık sonuç gösterimi |
| **100k Dosyalı Dizin Yükleme** | `< 300 ms` | Sanal liste renderlama |
| **Arayüz Tepki Hızı (UI FPS)**| `60 – 120 FPS`| Donma ve takılma yok |

---

## 6. Proje Dizin Yapısı (Önerilen)

```text
Projects/Firewol/
├── src-tauri/                  # Rust Backend (Tauri v2)
│   ├── src/
│   │   ├── main.rs
│   │   ├── search/             # NTFS MFT Parser, USN Journal, FTS5 Indexer
│   │   ├── fs_engine/          # Paralel transfer motoru, checksum, virtual fs
│   │   ├── shell_menu/         # Nilesoft düzeni ve Win32 context menu köprüsü
│   │   ├── elevation/          # UAC worker ve izin yöneticisi
│   │   └── config/             # Metadata DB (klasör renk/simge ve ayarlar)
│   └── Cargo.toml
├── src/                        # UI Katmanı (Svelte / React)
│   ├── components/
│   │   ├── explorer/           # Virtual scroll dosya listesi, breadcrumb
│   │   ├── panels/             # Split-pane ve gizlenebilir sidebar
│   │   ├── context_menu/       # Nilesoft tarzı özelleştirilebilir sağ tık
│   │   ├── palette/            # Yuvarlak tema renk paleti bileşeni
│   │   └── window_controls/    # Özelleştirilebilir pencere butonları
│   ├── themes/                 # 7 Morphism preset CSS/Tailwind şablonları
│   └── stores/                 # State ve ayar yönetimi
└── PROMPT.md                   # Proje Ana Yönerge ve Şartnamesi
```

---

## 7. Uygulama Talimatı (Prompt Directive)

Bu şartname doğrultusunda Firewol File Explorer projesini geliştirirken:
1. Her zaman belirtilen RAM bütçesini (tek pencere ~150 MB, en kötü durum < 250 MB) gözetin.
2. Hiçbir Windows kirliliğini (Home sekmesi zorunluluğu, istenmeyen quick-access yerleşimleri) projeye dahil etmeyin; arayüzü tertemiz ve minimal başlatın.
3. 7 farklı morphism temasını ve yuvarlak renk paleti düzenleyicisini eksiksiz destekleyin.
4. Nilesoft Shell esintili sağ tık menüsünü hem uygulama içinde tam özelleştirilebilir hem de isteğe bağlı OS entegrasyonuna hazır tasarlayın.
5. Rust MFT arama motorunu ve paralel dosya transfer sistemini veri güvenliğini (Recycle Bin / Undo) koruyarak inşa edin.
