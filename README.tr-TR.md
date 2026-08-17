# Win-CodexBar

[English](./README.md) | [简体中文](./README.zh-CN.md) | [繁體中文（臺灣）](./README.zh-TW.md) | [日本語](./README.ja-JP.md) | [한국어](./README.ko-KR.md) | [Español mexicano](./README.es-MX.md) | [Türkçe](./README.tr-TR.md)

Win-CodexBar, bir düzine pano açmadan yapay zekâ kodlama araçlarının kullanımını görebilmenizi sağlayan bir Windows sistem tepsisi uygulamasıdır. [CodexBar](https://github.com/steipete/CodexBar) ruhunu, ortak Rust sağlayıcı mantığıyla desteklenen Tauri + React masaüstü kabuğuna taşır.

<table>
  <tr>
    <td width="36%" align="center">
      <img src="docs/images/tray-panel.png" alt="Sağlayıcı kullanım kartlarını gösteren Win-CodexBar tepsi paneli"/>
    </td>
    <td width="64%" align="center">
      <img src="docs/images/settings-providers.png" alt="Win-CodexBar Sağlayıcılar ayar sayfası"/>
    </td>
  </tr>
</table>

## Öne Çıkanlar

- Codex, Claude, Copilot, OpenRouter, Cursor, Gemini, DeepSeek, MiniMax, Kiro, Antigravity, Groq, Qoder, Sakana AI, CrossModel ve daha fazlası dahil **56 sağlayıcı**.
- Kompakt sağlayıcı ızgarası, kullanım kartları, yenileme işlemi, ayarlar kısayolu ve çıkış denetimiyle **tepsi odaklı iş akışı**.
- Kaynak seçimi, kimlik bilgileri, çerez içe aktarma, token hesapları, API anahtarları, bölgeler ve tepsi görünüm tercihleri için **sağlayıcı ayarları**.
- Uygulama tarafından yönetilen API anahtarları, elle eklenen çerezler ve token hesapları için, kullanılabildiğinde kullanıcı kapsamındaki DPAPI'den yararlanan **Windows kimlik bilgisi koruması**.
- Sağlayıcı başına isteğe bağlı **Chrome, Edge, Brave ve Firefox çerezlerini içe aktarma**.
- Kullanım, maliyet, yapılandırma, tanılama ve geri döngü entegrasyonlarını betiklemek için **kurulu yerel CLI**.
- WebView2 ve VC++ çalışma zamanı önyüklemesi ile SHA-256 sağlama toplamı dosyaları içeren **yükleyici ve taşınabilir derlemeler**.

## Kurulum

Windows Paket Yöneticisi ile kurun:

```powershell
winget install Finesssee.Win-CodexBar
```

Alternatif olarak en son yükleyiciyi veya taşınabilir derlemeyi [GitHub Releases](https://github.com/Finesssee/Win-CodexBar/releases) sayfasından indirebilirsiniz.

- Yükleyici: `CodexBar-<sürüm>-Setup.exe`
- Taşınabilir: `CodexBar-<sürüm>-portable.exe`
- Sağlama toplamları: her sürüm `.sha256` dosyaları içerir

Winget dağıtımı [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs/tree/master/manifests/f/Finesssee/Win-CodexBar) aracılığıyla onaylanmıştır. Her Winget güncellemesi belirli bir sürüm URL'sine ve yükleyici özetine sabitlendiği için yeni sürümlerin görünmesi biraz zaman alabilir.

## Kod İmzalama

> **Kod imzalama:** SignPath.io üzerinden ücretsiz imzalama (sertifika: SignPath Foundation) **planlanmıştır, katılım beklenmektedir ve henüz sürüm işlem hattına bağlanmamıştır**. İmzalama ilkeleri için [docs/CODE_SIGNING.md](docs/CODE_SIGNING.md) belgesine bakın.
> Windows sürüm yükleyicileri şu anda imzasızdır; bu durum hatalı bir SmartScreen/Defender uyarısına neden olabilir. Sürümle birlikte yayımlanan SHA-256 değerini doğrulayın; veri işleme bilgileri için [docs/PRIVACY.md](docs/PRIVACY.md) belgesine bakın.

## İlk Çalıştırma

1. Başlat Menüsü'nden veya taşınabilir çalıştırılabilir dosyadan **CodexBar**'ı başlatın.
2. Kullanım panelini açmak için tepsi simgesine tıklayın.
3. **Ayarlar -> Sağlayıcılar**'ı açın.
4. Kullandığınız sağlayıcıları etkinleştirin.
5. Uygun kimlik bilgisi türünü ekleyin: OAuth/cihazla oturum açma, API anahtarı, tarayıcı çerezleri, yerel CLI oturumu veya token hesabı.

Claude için ayarlar sayfasındaki kullanımla eşleştikleri için tarayıcı çerezleri/sessionKey tercih edilir. OAuth ve CLI yedek seçenek olarak kullanılabilir. Codex ve Gemini gibi CLI tabanlı sağlayıcılarda önce sağlayıcının CLI aracında oturum açın.

## En Son Sürüm

**v0.33.2**, tepsi panelinin odak kaybında veya Escape tuşuna basıldığında kapanmasını ve aynı tepsi tıklamasıyla hemen yeniden açılmamasını sağlar.

Tüm geçmişi [CHANGELOG.md](CHANGELOG.md) dosyasında görebilirsiniz.

## Desteklenen Sağlayıcılar

<details>
<summary>Sağlayıcı matrisi</summary>

| Sağlayıcı | Kimlik Doğrulama | İzlenenler |
|---|---|---|
| Codex | OAuth / CLI | Oturum, Haftalık, Krediler |
| Claude | Çerezler / OAuth yedeği / CLI yedeği | Oturum (5 sa), Haftalık |
| Cursor | Çerezler | Plan, Kullanım, Faturalandırma |
| Factory | Çerezler | Kullanım |
| Gemini | gcloud OAuth | Kota |
| Copilot | GitHub Cihaz Akışı / gh CLI / eski token | Plan kullanımı, Sohbet |
| Antigravity | Yerel LSP | Kullanım, Model başına kotalar |
| z.ai | API Tokenı | Kota |
| MiniMax | API / Çerezler | Kullanım, Faturalandırma Özeti |
| Kiro | Çerezler / CLI | Aylık Krediler, Fazla Kullanım |
| Vertex AI | gcloud OAuth | Maliyet |
| Augment | Çerezler | Krediler |
| OpenCode | Yerel Yapılandırma | Kullanım |
| Kimi | Çerezler | 5 saatlik Hız, Haftalık |
| Kimi K2 | API Anahtarı | Krediler |
| Amp | Çerezler | Kullanım |
| Warp | Yerel Yapılandırma | Kullanım |
| Ollama | Çerezler / API Anahtarı | Kullanım, Bulut Modelleri, Hız pencereleri |
| Azure OpenAI | API Anahtarı | Dağıtım |
| T3 Chat | Çerezler / cURL | Temel, Fazla Kullanım |
| OpenRouter | API Anahtarı | Krediler |
| JetBrains AI | Yerel Yapılandırma | Kullanım |
| Alibaba | Çerezler | Kullanım |
| Alibaba Token Plan | Çerezler | Token Planı Kredileri, Sıfırlanma tarihi |
| NanoGPT | API Anahtarı | Krediler |
| Infini | API Anahtarı | Oturum, Haftalık, Kota |
| Perplexity | Çerezler | Krediler, Plan |
| Abacus AI | Çerezler | Krediler |
| Mistral | Çerezler | Faturalandırma, Kullanım |
| OpenCode Go | Çerezler | Kullanım, Zen Bakiyesi |
| Kilo | API Anahtarı / CLI | Kullanım |
| Codebuff | API Anahtarı / Yerel Yapılandırma | Krediler, Haftalık |
| DeepSeek | API Anahtarı | Bakiye, Kullanım özetleri, Maliyet |
| Windsurf | Yerel Önbellek | Günlük, Haftalık |
| Manus | Çerezler | Krediler, Yenileme Kredileri |
| Xiaomi MiMo | Çerezler | Bakiye, Token Planı |
| Doubao | API Anahtarı | İstek Sınırları |
| Command Code | Çerezler | Aylık Krediler, Satın Alınan Krediler |
| Crof | API Anahtarı | Krediler, İstek Kotası |
| StepFun | Oasis Tokenı | 5 saatlik, Haftalık, Token yenileme |
| Venice | API Anahtarı | USD / DIEM Bakiyesi |
| OpenAI | Yönetici API'si / API Anahtarı | Kullanım, İstekler, Proje kapsamlı maliyet, Kredi Bakiyesi |
| Grok | Çerezler / auth.json | Faturalandırma |
| ElevenLabs | API Anahtarı | Abonelik Kredileri, Ses Yuvaları |
| Deepgram | API Anahtarı | Proje Kullanımı |
| Groq | API Anahtarı | Kurumsal Ölçümler |
| LLM Proxy | API Anahtarı | Kota İstatistikleri |

</details>

## Desteklenen Diller

Arayüz ve katkıda bulunanlara yönelik raporlama şu dilleri destekler:

- English
- 简体中文
- 繁體中文（臺灣）
- 日本語
- 한국어
- Español mexicano
- Русский
- Türkçe

## Kaynaktan Derleme

```powershell
# Gereksinimler: Node.js + pnpm. Rust ve MinGW gerektiğinde betik tarafından kurulur.
git clone https://github.com/Finesssee/Win-CodexBar.git
cd Win-CodexBar
.\scripts\dev.ps1
```

Yararlı geliştirme seçenekleri:

```powershell
.\scripts\dev.ps1 -Release      # iyileştirilmiş derleme
.\scripts\dev.ps1 -SkipBuild    # son derlemeyi yeniden başlat
```

CLI örnekleri:

```bash
codexbar-cli --help
codexbar-cli diagnose --pretty
codexbar-cli usage -p claude
codexbar-cli usage -p all
codexbar-cli cost -p codex
```

Yükleyici derlemeleri tepsi uygulaması olarak `codexbar.exe` ve konsol CLI aracı olarak `codexbar-cli.exe` içerir. Başlat Menüsü kısayolları masaüstü uygulamasını, terminal komutları `codexbar-cli.exe` dosyasını başlatır. `codexbar-desktop.exe`, eski kısayollar ve otomatik başlatma girdileri için uyumluluk takma adı olarak kurulmaya devam eder.

## Sürüm Derlemeleri

Yerel Windows sürüm derlemeleri için önbellekli sürüm oluşturucuyu kullanın:

```powershell
.\scripts\windows-release-build.ps1 -Ref v0.33.2 -SmokeInstall
```

Betik gerçek Tauri sürüm ikilisini ve konsol CLI aracını derler, imzalı yükleyici bağımlılıklarını doğrular, Inno Setup ile paketler, yükleyici/taşınabilir varlıkları ve SHA-256 yan dosyalarını oluşturur ve sessiz kurulum/kaldırma duman testi çalıştırabilir.

Daha fazla sürüm otomasyonu bilgisi [docs/release/ci-cd.md](docs/release/ci-cd.md) belgesindedir.

## Gizlilik

- **Varsayılan olarak cihazda**: sağlayıcı verileri bilinen yerel yollardan veya yapılandırdığınız sağlayıcı API'lerinden okunur.
- **İsteğe bağlı çerezler**: tarayıcı çerezi çıkarma yalnızca etkinleştirdiğiniz sağlayıcılar için çalışır.
- **Korumalı sırlar**: API anahtarları, elle eklenen çerezler ve token hesapları güvenli dosya katmanını kullanır; Windows kullanılabildiğinde kullanıcı kapsamındaki DPAPI'yi kullanır.
- **Güvenli tanılama**: tanılama yalnızca sağlayıcı/kaynak/durum meta verilerini gösterir; ham çerezleri, API anahtarlarını, taşıyıcı tokenlarını veya OAuth değerlerini asla göstermez.
- **Doğrulanmış güncellemeler**: yükleyici indirmeleri GitHub SHA-256 özeti gerektirir ve uygulanmadan hemen önce yeniden doğrulanır.

## Belgeler

| Konu | Bağlantı |
|---|---|
| Kaynaktan derleme | [docs/BUILDING.md](docs/BUILDING.md) |
| WSL kurulumu ve kimlik doğrulama ipuçları | [docs/WSL.md](docs/WSL.md) |
| Tarayıcı çerez ayrıntıları | [docs/COOKIES.md](docs/COOKIES.md) |

## Katkıda Bulunanlar

- Orijinal macOS uygulaması: Peter Steinberger tarafından [steipete/CodexBar](https://github.com/steipete/CodexBar)
- Maliyet takibi için [ccusage](https://github.com/ryoppippi/ccusage) projesinden esinlenilmiştir

## Lisans

Orijinal CodexBar ile aynı olan MIT Lisansı.
