# Win-CodexBar

[English](./README.md) | [简体中文](./README.zh-CN.md) | [繁體中文（臺灣）](./README.zh-TW.md) | [日本語](./README.ja-JP.md) | [한국어](./README.ko-KR.md) | [Español mexicano](./README.es-MX.md) | [Türkçe](./README.tr-TR.md)

Win-CodexBar は、複数の AI コーディングツールの使用量を Windows のシステムトレイから確認できるデスクトップアプリです。[CodexBar](https://github.com/steipete/CodexBar) の考え方を、Tauri + React のデスクトップシェルと共有 Rust バックエンドで Windows 向けに移植しています。

<table>
  <tr>
    <td width="36%" align="center">
      <img src="docs/images/tray-panel.png" alt="プロバイダー使用量カードを表示する Win-CodexBar のトレイパネル"/>
    </td>
    <td width="64%" align="center">
      <img src="docs/images/settings-providers.png" alt="Win-CodexBar のプロバイダー設定画面"/>
    </td>
  </tr>
</table>

## 主な機能

- **56 プロバイダー**: Codex、Claude、Copilot、OpenRouter、Cursor、Gemini、DeepSeek、MiniMax、Kiro、Antigravity、Groq、Qoder、Sakana AI、CrossModel など。
- **トレイ中心のワークフロー**: コンパクトなプロバイダー一覧、使用量カード、更新、設定、終了操作。
- **プロバイダー設定**: ソース選択、認証情報、Cookie インポート、トークンアカウント、API キー、リージョン、トレイ表示設定。
- **Windows 認証情報保護**: アプリ管理の API キー、手動 Cookie、トークンアカウントを、利用可能な場合はユーザー単位の DPAPI で保護。
- **ブラウザー Cookie インポート**: Chrome、Edge、Brave、Firefox に対応。プロバイダーごとに明示的に有効化します。
- **ローカル CLI**: 使用量、コスト、設定、診断、ローカル連携をスクリプトから利用できます。
- **インストーラー / ポータブルビルド**: WebView2 Runtime、VC++ Runtime、SHA-256 チェックサムを含みます。

## インストール

Windows Package Manager でインストールできます。

```powershell
winget install Finesssee.Win-CodexBar
```

または [GitHub Releases](https://github.com/Finesssee/Win-CodexBar/releases) から最新のインストーラー / ポータブル版をダウンロードしてください。

- インストーラー: `CodexBar-<version>-Setup.exe`
- ポータブル: `CodexBar-<version>-portable.exe`
- チェックサム: 各リリースに `.sha256` ファイルが含まれます

## 初回起動

1. スタートメニューまたはポータブル exe から **CodexBar** を起動します。
2. トレイアイコンをクリックして使用量パネルを開きます。
3. **Settings -> Providers** を開きます。
4. 使うプロバイダーを有効化します。
5. OAuth / デバイスログイン、API キー、ブラウザー Cookie、ローカル CLI ログイン、トークンアカウントなど、必要な認証情報を追加します。

Claude では、Claude の設定ページと同じ使用量に近づけるため、ブラウザー Cookie / sessionKey が優先です。OAuth と CLI はフォールバックとして利用できます。Codex や Gemini など CLI ベースのプロバイダーは、先に各プロバイダーの CLI でログインしてください。

## 最新リリース

現在の変更履歴は [CHANGELOG.md](CHANGELOG.md) を参照してください。対応プロバイダーの一覧は [English README](./README.md#supported-providers) にあります。

## ソースからビルド

```powershell
# 前提: Node.js + pnpm。Rust と MinGW は必要に応じてスクリプトがインストールします。
git clone https://github.com/Finesssee/Win-CodexBar.git
cd Win-CodexBar
.\scripts\dev.ps1
```

便利な開発フラグ:

```powershell
.\scripts\dev.ps1 -Release      # 最適化ビルド
.\scripts\dev.ps1 -SkipBuild    # 直前のビルドを再起動
```

CLI 例:

```bash
codexbar-cli --help
codexbar-cli diagnose --pretty
codexbar-cli usage -p claude
codexbar-cli usage -p all
codexbar-cli cost -p codex
```

## プライバシー

- **基本はローカル**: プロバイダーデータは既知のローカルパス、または設定したプロバイダー API から読み取ります。
- **Cookie は任意**: ブラウザー Cookie 抽出は、有効化したプロバイダーに対してのみ実行されます。
- **シークレット保護**: API キー、手動 Cookie、トークンアカウントはセキュアファイル層で保存され、Windows では利用可能な場合 DPAPI を使います。
- **安全な診断**: 診断はプロバイダー、ソース、状態のメタデータのみを表示し、Cookie、API キー、Bearer トークン、OAuth 値は表示しません。

## ドキュメント

| トピック | リンク |
|---|---|
| ソースからビルド | [docs/BUILDING.md](docs/BUILDING.md) |
| WSL セットアップと認証 Tips | [docs/WSL.md](docs/WSL.md) |
| ブラウザー Cookie の詳細 | [docs/COOKIES.md](docs/COOKIES.md) |

## クレジット

- 元の macOS アプリ: Peter Steinberger 氏の [steipete/CodexBar](https://github.com/steipete/CodexBar)
- コスト追跡は [ccusage](https://github.com/ryoppippi/ccusage) から着想を得ています

## ライセンス

元の CodexBar と同じ MIT ライセンスです。
