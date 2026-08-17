# Win-CodexBar

[English](./README.md) | [简体中文](./README.zh-CN.md) | [繁體中文（臺灣）](./README.zh-TW.md) | [日本語](./README.ja-JP.md) | [한국어](./README.ko-KR.md) | [Español mexicano](./README.es-MX.md) | [Türkçe](./README.tr-TR.md)

Win-CodexBar는 여러 AI 코딩 도구의 사용량을 Windows 시스템 트레이에서 바로 확인할 수 있게 해 주는 데스크톱 앱입니다. [CodexBar](https://github.com/steipete/CodexBar)의 아이디어를 Tauri + React 데스크톱 셸과 공유 Rust 백엔드로 Windows에 맞게 포팅했습니다.

<table>
  <tr>
    <td width="36%" align="center">
      <img src="docs/images/tray-panel.png" alt="프로바이더 사용량 카드를 보여 주는 Win-CodexBar 트레이 패널"/>
    </td>
    <td width="64%" align="center">
      <img src="docs/images/settings-providers.png" alt="Win-CodexBar 프로바이더 설정 화면"/>
    </td>
  </tr>
</table>

## 주요 기능

- **56개 프로바이더**: Codex, Claude, Copilot, OpenRouter, Cursor, Gemini, DeepSeek, MiniMax, Kiro, Antigravity, Groq, Qoder, Sakana AI, CrossModel 등.
- **트레이 중심 워크플로**: 작은 프로바이더 그리드, 사용량 카드, 새로고침, 설정 바로가기, 종료 컨트롤.
- **프로바이더별 설정**: 소스 선택, 인증 정보, 쿠키 가져오기, 토큰 계정, API 키, 리전, 트레이 표시 설정.
- **Windows 인증 정보 보호**: 앱이 관리하는 API 키, 수동 쿠키, 토큰 계정을 가능한 경우 사용자 범위 DPAPI로 보호합니다.
- **브라우저 쿠키 가져오기**: Chrome, Edge, Brave, Firefox를 지원하며 프로바이더별로 명시적으로 켜야 합니다.
- **로컬 CLI**: 사용량, 비용, 설정, 진단, 로컬 루프백 연동을 스크립트에서 사용할 수 있습니다.
- **설치형 / 포터블 빌드**: WebView2 Runtime, VC++ Runtime 부트스트랩, SHA-256 체크섬 파일을 포함합니다.

## 설치

Windows Package Manager로 설치할 수 있습니다.

```powershell
winget install Finesssee.Win-CodexBar
```

또는 [GitHub Releases](https://github.com/Finesssee/Win-CodexBar/releases)에서 최신 설치 파일이나 포터블 빌드를 내려받으세요.

- 설치 파일: `CodexBar-<version>-Setup.exe`
- 포터블: `CodexBar-<version>-portable.exe`
- 체크섬: 각 릴리스에 `.sha256` 파일 포함

## 처음 실행

1. 시작 메뉴 또는 포터블 실행 파일에서 **CodexBar**를 실행합니다.
2. 트레이 아이콘을 클릭해 사용량 패널을 엽니다.
3. **Settings -> Providers**를 엽니다.
4. 사용하는 프로바이더를 켭니다.
5. OAuth / 디바이스 로그인, API 키, 브라우저 쿠키, 로컬 CLI 로그인, 토큰 계정 등 필요한 인증 방식을 추가합니다.

Claude는 Claude 설정 페이지의 사용량과 맞추기 위해 브라우저 쿠키 / sessionKey를 우선 사용합니다. OAuth와 CLI는 폴백으로 남아 있습니다. Codex나 Gemini처럼 CLI 기반인 프로바이더는 먼저 해당 프로바이더 CLI에서 로그인하세요.

## 최신 릴리스

전체 변경 내역은 [CHANGELOG.md](CHANGELOG.md)를 확인하세요. 지원 프로바이더 목록은 [English README](./README.md#supported-providers)에 있습니다.

## 소스에서 빌드

```powershell
# 사전 요구 사항: Node.js + pnpm. Rust와 MinGW는 필요할 때 스크립트가 설치합니다.
git clone https://github.com/Finesssee/Win-CodexBar.git
cd Win-CodexBar
.\scripts\dev.ps1
```

유용한 개발 플래그:

```powershell
.\scripts\dev.ps1 -Release      # 최적화 빌드
.\scripts\dev.ps1 -SkipBuild    # 마지막 빌드 다시 실행
```

CLI 예시:

```bash
codexbar-cli --help
codexbar-cli diagnose --pretty
codexbar-cli usage -p claude
codexbar-cli usage -p all
codexbar-cli cost -p codex
```

## 개인정보 보호

- **기본은 로컬**: 프로바이더 데이터는 알려진 로컬 경로나 사용자가 설정한 프로바이더 API에서 읽습니다.
- **쿠키는 선택 사항**: 브라우저 쿠키 추출은 켠 프로바이더에 대해서만 실행됩니다.
- **비밀 정보 보호**: API 키, 수동 쿠키, 토큰 계정은 보안 파일 계층에 저장되며 Windows에서는 가능한 경우 DPAPI를 사용합니다.
- **안전한 진단**: 진단은 프로바이더, 소스, 상태 메타데이터만 표시하고 쿠키, API 키, bearer 토큰, OAuth 값은 표시하지 않습니다.

## 문서

| 주제 | 링크 |
|---|---|
| 소스에서 빌드 | [docs/BUILDING.md](docs/BUILDING.md) |
| WSL 설정과 인증 팁 | [docs/WSL.md](docs/WSL.md) |
| 브라우저 쿠키 상세 | [docs/COOKIES.md](docs/COOKIES.md) |

## 크레딧

- 원본 macOS 앱: Peter Steinberger의 [steipete/CodexBar](https://github.com/steipete/CodexBar)
- 비용 추적은 [ccusage](https://github.com/ryoppippi/ccusage)에서 영감을 받았습니다

## 라이선스

원본 CodexBar와 같은 MIT 라이선스입니다.
