# Firewol — Windows dosya yöneticisi planı

## İlk sürüm

Windows 11 x64 için açık kaynaklı, standart kullanıcı yetkisiyle çalışan bir dosya yöneticisi geliştir. İlk kullanılabilir sürümde sade gezinme, sanal dosya listesi, güvenli temel dosya işlemleri, seçili klasör araması ve **UAC onayıyla seçilen NTFS bölümünün tamamında arama** yer alsın. İlk açılışta kullanıcı klasörünü göster, sonraki açılışlarda son geçerli konumu kullan. Kenar çubuğunu kullanıcının elle eklediği sabitlerle oluştur.

Ana görünüm ve Firewol içi sağ tık menüsü için Nilesoft ekran görüntülerindeki nötr yüzeyleri, kompakt ikonlu satırları, ince grup ayraçlarını ve hizalı klavye kısayollarını referans al. Sağ tık özelleştirmesinin kapsamı Firewol içiyle sınırlı. [Koyu görünüm](https://nilesoft.org/images/screenshots/s1.png) · [Açık menü](https://nilesoft.org/images/screenshots/light/goto.png)

## Uygulama sırası

1. **M0 — Windows prototipi ve bellek kararı.** Rust çekirdeğini arayüzden ayır. Tek WebView2 kullanan Tauri v2, Svelte ve TypeScript ile listeleme, indeks sorgusu ve NTFS yardımcısını içeren bir ölçüm prototipi kur. Toplam bellek 150 MB sınırına ulaşırsa aynı çekirdeği `windows-rs`, Win32 sanal liste ve Direct2D ile kullan. Bu makinede Rust, Node ve WebView2 bulundu. MSVC x64 ve Windows SDK, ilk derlemeden önce doğrulanacak. Bu aşamada Geri Dönüşüm Kutusu’na gönderme ve aynı öğeyi geri yükleme de gerçek Windows üzerinde sınanacak. [Tauri önkoşulları](https://v2.tauri.app/start/prerequisites/) · [WebView2 süreç modeli](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/process-model)
2. **M1 — İlk kullanılabilir sürüm.** Adres çubuğu, klasör gezinmesi, temel sağ tık menüsü, arama ve dosya işlemlerini birleştir. Arayüz `list_dir`, `search`, `submit_operation` ve `undo` için tipli Rust sözleşmelerini kullansın. Liste ve sonuçlar sayfalı, sorgular iptal edilebilir olsun. Dosya işlemleri ekrandaki metin yerine çekirdeğin verdiği öğe kimliğini kullansın.
3. **M2 — Gezinme ve görünüm.** Sürüklenebilir sekmeleri, çift paneli, paneller arası aktarımı, gizlenebilir kenar çubuğunu ve düzenlenebilir sağ tık menüsünü ekle. Renk çemberini, yedi temayı ve özelleştirilebilir pencere düğmelerini ortak tema değerleriyle kur. Klasör takma adı, renk, simge ve etiketleri `%LOCALAPPDATA%\Firewol` altında, arama indeksinden ayrı sakla. Ayrı pencereye taşınan sekmeleri de bellek ölçümüne dahil et.
4. **M3 — Transfer ve yetki seçenekleri.** Düzenli yerel dosyalar için sınırlı paralellik, hız grafiği ve `CopyFile2` ile duraklatma/devam ekle. Geçici hedefi sağlama toplamıyla doğruladıktan sonra yayımla. Diskler arası taşımada kaynağı ancak doğrulama ve hedefe yerleştirme tamamlanınca kaldır. Yönetici işlem ayarları, izin listesi veya uyarılı geniş kapsam üzerinden hangi eylemlerin UAC isteyebileceğini belirlesin. [CopyFile2](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-copyfile2) · [Duraklatma ve devam](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ne-winbase-copyfile2_message_action)

## Arama ve dosya işlemleri

- **İndeks.** Disk tabanlı SQLite FTS5 kullan. Bellek içi denemede üç karakterlik alt dize, uzantı filtresi ve iki karakterlik önek sorgusu doğrulandı. Bir ve iki karakterde önek, üç ve üzerindeyse dosya adı içinde eşleşme kullan. Boyut, uzantı ve tarih filtrelerini SQL’de uygula. Regex sonuçlarını iptal edilebilir gruplar halinde göster. [SQLite FTS5](https://www.sqlite.org/fts5.html)
- **NTFS.** Yalnızca kullanıcı etkinleştirdiğinde, salt okunur ve oturuma bağlı bir UAC yardımcısı `FSCTL_ENUM_USN_DATA` ile MFT kayıtlarını alsın. Yardımcı, istemcinin kullanıcı yetkileriyle görünürlüğü doğrulanan girdileri yerel, erişimi kısıtlı bir kanal üzerinden iletsin. İlk taramadan önce USN konumunu al, tarama sırasında gelen değişiklikleri uygula ve indeksle imleci aynı işlemde kaydet. Journal sıfırlanırsa veya kayıtlar kaçırılırsa ilgili kapsamı yeniden kur. Standart klasör araması dizin taraması ve değişiklik bildirimleriyle çalışsın. [Win32 bölüm erişimi](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew) · [MFT numaralandırma](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_enum_usn_data) · [USN günlüğü](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_read_usn_journal)
- **Geri al.** Kopyalama, taşıma ve yeniden adlandırmada `IFileOperation`, silmede Geri Dönüşüm Kutusu’na taşıyan `ITransferSource::RecycleItem` kullan. `Ctrl+Z`, oturumda başarıyla tamamlandığı doğrulanmış ve güvenle tersine çevrilebilen öğeleri işlesin. Geri yüklemede tam öğe kimliğini doğrula. Çakışma veya geri dönüşüm hatasında işlemi durdur ve kullanıcıya bildir. [IFileOperation](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nn-shobjidl_core-ifileoperation) · [RecycleItem](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-itransfersource-recycleitem)

## Kabul ölçütleri

- Release derlemesini 100.000 öğeli tek klasör ve 1 milyon indeks kaydı içeren sentetik NTFS diskte ölç. Firewol, WebView2 ve yardımcı süreçlerin **toplam tepe özel çalışma kümesi 150.000.000 baytın altında** kalmalı. Başlangıç, boşta, kaydırma, indeksleme ve transfer sırasında ölç. Boşta 40–70 MB hedefle. Hazır indekste ilk görünür arama sonucu p95 `<100 ms`, büyük dizinde ilk görünür liste p95 `<300 ms` ve kaydırma en az 60 FPS olmalı. İlk indeksleme ve tam sıralama sürelerini ayrı kaydet. [Windows bellek sayacı](https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-process_memory_counters_ex2)
- Windows üzerinde UAC reddini, erişilemeyen klasörleri, journal kaybını, uzun ve Unicode yolları, kesilen transferi ve geri alma çakışmasını dene. Rust biçim kontrolü, Clippy, testler ve seçilen arayüzün tip kontrolü temiz geçsin. Başlangıç lisansı olarak MIT kullan.

## Sohbet devri

Plan Modu nedeniyle dosyaların kaydı uygulama turuna kaldı. İlk adım bu planı kaynaklarıyla birlikte İngilizce `PLAN.md` olarak kaydetmek ve Firewol köküne aşağıdaki `STATE.md` dosyasını eklemek.

```markdown
# Firewol State

## Purpose
Build a lightweight, customizable Windows 11 file manager with instant local search.

## Currently Working State & Constraints
- PROMPT.md holds the product spec. PLAN.md holds the execution plan and sources.
- Research is complete. Bootstrap and runtime measurements are pending.
- The first usable release includes folder search and opt-in, read-only NTFS volume indexing.
- Keep the total private working set below 150 MB. Try Tauri first and switch to native Win32 if it exceeds the cap.
- Nilesoft screenshots guide the default appearance and in-app menu. Store customization in local app data.

## Next Immediate Step
Run M0 from PLAN.md. Verify MSVC and the Windows SDK, then measure the prototype and prove recycle/restore on synthetic NTFS data.
```
