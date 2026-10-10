# OpenMouse EX-G PRO for macOS (experimental)

EX-G PROの追加ボタン6〜8を、Command/Control/Option/Shiftと通常キーの組み合わせに割り当てる、独立した小さなヘルパーです。例：⌘Z、⌘C、⌘⇧Z。通常ボタン1〜5、移動、スクロールは変更しません。

v0.2.0では「Mission Control」「アプリケーションのウインドウ」「デスクトップを表示」を選べます。F11などの通常キー送信とは区別します。保存済みのv0.1.0のショートカット設定も読み込めます。旧版を起動中なら、先に専用画面の「常駐処理を終了」で閉じてから新版を開いてください。

**Mouse Assistant 6.4.0の機種定義に基づく実装です。購入した実機は未検証です。** 設定をマウスの本体へ書き込む方式ではなく、このMacでのソフトウェア変換です。通常版OpenMouse Bridgeを起動する必要はありません。

## 起動・設定・終了

1. ビルドした `OpenMouse EX-G PRO.app` を開くと、ブラウザに `http://127.0.0.1:17847` の専用画面が開きます。メニューバー、Dock、ログイン項目は追加しません。
2. 「macOSの許可設定を開く」から、入力監視とアクセシビリティにこのアプリを許可します。OSの許可変更後は「常駐処理を終了」で終了し、アプリを開き直します。必要ならシステム設定で `.app` を手動追加します。
3. 機種を選び、各追加ボタンのキーと修飾キーを選びます。ZとCommandで⌘Z。チェックを有効にして「保存して適用」を押します。初回は割り当て無効です。
   ウインドウ操作はドロップダウンの「macOSの操作」から選びます。Mission Controlは全体の一覧、アプリケーションのウインドウは現在のアプリの一覧、デスクトップを表示はウインドウを退ける操作です。この3操作では修飾キーを設定しません。
4. ボタン番号は実際に押して「最後に押した追加ボタン」で確認します。これは物理位置の名称ではなくHIDボタン番号です。
5. ブラウザを閉じても変換は続きます。「常駐処理を終了」でヘルパー自体が終了します。再びアプリを開くと画面が開きます。自動起動はしません。

設定保存先：`~/Library/Application Support/OpenMouse/elecom.json`。接続方式を変えても同じ機種の割り当てを使います。再起動で有効設定を復元します。破損・不正な設定は起動時に拒否し、勝手にキーを送信しません。

## 処理と通信

- ネイティブヘルパー1プロセス。IOHIDの入力コールバックとローカルHTTPサーバーだけを常駐させます。既存Bridge本体の依存・起動処理を使用しない独立crateです。
- macOSの3操作は、Apple標準の `/System/Applications/Mission Control.app/Contents/MacOS/Mission Control` を固定の引数0/1/2で実行します。この小さな標準プログラムはDockへの要求を送って即終了し、追加のプロセスを常駐させません。OS側のショートカット設定やF11のキー設定に依存しません。シェル、AppleScript、プライベートframeworkへのリンク、自由なコマンド実行は追加していません。
- 対象Vendor/Product/PrimaryUsageだけを非排他的に開きます。マウス全体の占有、イベントタップ、キーボード監視、アプリ監視、HID設定書込みはありません。
- 追加ボタンの立ち上がりでキーのdown/upを1回送り、押しっぱなしのリピート、長押しマクロ、修飾キーのみの割り当てはしません。最初のレポートを初期状態として扱うので、接続直後に既に押されているボタンは一度離すまで動作させません。最初のレポートが最初の押下で届く機種では、その最初の押下も送信せず、離して再押下が必要です。
- 入力イベント、利用アプリ、キー入力をファイルに記録しません。保存するのは割り当て設定だけです。画面には直近の追加ボタン番号だけを表示します。
- 外向きの接続、テレメトリー、自動更新、ゲーム検出、バッテリー監視、Electron、LaunchAgentはありません。待受はIPv4 `127.0.0.1:17847` のみ。APIは正確なHost/Originを検証します。
- 専用UIはローカルJS/CSSとシステムフォントのみ。CSPで通信先を同一オリジンに制限します。通常版OpenMouse画面には既存の外部通信機能が残るため、無通信で使う場合は専用画面を使ってください。
- 画面を表示している間だけ、2秒ごとにローカルで接続状態を更新します。ブラウザを閉じると停止します。ネイティブ側でマウスの定期読み取りや権限確認のタイマーは使用しません。
- UI終了、Ctrl+C、SIGTERMでHTTPとネイティブ入力監視を停止します。二重起動は同じポートの排他で防ぎます。

## 対象機種と未検証事項

| Vendor | Product | メーカー機種定義 | 接続 | Report ID |
| --- | --- | --- | --- | --- |
| 056e | 013d | M-XGx50MB | USB受信機 | 1 |
| 056e | 013e | M-XGx50MB | Bluetooth | 2 |
| 056e | 0128 | M-XPT1MR | 有線 | 1 |
| 056e | 0129 | M-XPT1MR | USB受信機 | 1 |
| 056e | 0130 | M-XPT1MR | Bluetooth | 1 |

マウスコレクションはUsage Page 1 / Usage 2に限定します。Report ID付き8byte、または別引数でIDを受ける7byte payloadだけを受理します。メーカー定義はIDの次の1byteを8ボタンのビットマスクとしており、追加ボタン6/7/8はbit5/6/7です。

実機ではProduct ID一致、各ボタンのビット、macOS IOHIDから届く長さとReport ID、Bluetooth再接続、押下/解放、ショートカットの実行、通常クリックやスクロールの影響を確認する必要があります。登録外IDや不一致パケットは無視します。ファームウェア初期設定や別モデルが同じ挙動であるとは保証しません。

この実装はOSの元の追加クリックを抑止しません。アプリがボタン6〜8にも独自の動作を持つ場合、ショートカットと重複する可能性があります。メーカーアプリと同時起動すると二重変換になる可能性があります。イベントタップによる抑止は現在の実装範囲に含めていません。チルト左右は今回の割り当て対象に含めていません。

## ソースからビルド

macOS、Node.js 22以上、Rust 1.88以上/Cargo、Appleのコマンドラインツールが必要です。GitHubの `feat/elecom-exg-pro` ブランチを取得します。

```sh
git clone -b feat/elecom-exg-pro https://github.com/Aoki-lab1110/openmouse.git app
git clone -b feat/elecom-exg-pro https://github.com/Aoki-lab1110/OpenMouse-Bridge.git bridge
cd app
npm ci --ignore-scripts --no-audit --no-fund
npm run build:elecom
cd ../bridge
sh elecom-helper/bundle-macos.sh ../app/dist-elecom
```

ビルド時の依存取得はネットワークを使用します。実行時は使用しません。アプリはローカルのad-hoc署名です。Developer ID署名・公証版ではありません。配布用に公証した成果物は作成していません。

CLIの場合：

```sh
cargo run --locked --release --manifest-path elecom-helper/Cargo.toml -- --ui-dir ../app/dist-elecom
```

`--config PATH` で設定先を変更できます。CLIではブラウザを自動で開きません。UIを開かなくても保存済みの割り当てを処理します。

## 検証とライセンス

```sh
cargo test --locked --manifest-path elecom-helper/Cargo.toml
cargo clippy --locked --manifest-path elecom-helper/Cargo.toml --all-targets -- -D warnings
```

テストは入力デコード、不正設定の拒否、ボタンの立ち上がり、通常ボタンの除外、Host/Origin検証を対象にします。UIの⌘Z/⌘⇧Z/⌘C設定と永続化、終了もローカルで確認しました。マウス実機からの入力とショートカット実行は未確認です。

v0.2.0のテストには、保存済みキーボード設定の互換性、3種類のmacOS操作のJSON保存・復元、長押しでの重複防止、未知の操作名や混在した設定・自由なコマンドの拒否も含めます。標準プログラムの引数の対応は、このMacのApple提供バイナリを読み取り専用で確認しました。標準プログラムの引数は公開APIとして保証されていないため、将来のmacOSで変わる可能性があります。実際のマウスからの操作は未検証です。

macOSのウインドウ操作と通常のショートカットについては [Appleユーザガイド](https://support.apple.com/ja-jp/guide/mac-help/mh35798/mac) を参照できます。

このcrateはMIT。OpenMouseのUIはAGPL-3.0-or-laterです。対応する変更済みUIソースは [openmouse fork](https://github.com/Aoki-lab1110/openmouse/tree/feat/elecom-exg-pro)、ネイティブソースは [Bridge fork](https://github.com/Aoki-lab1110/OpenMouse-Bridge/tree/feat/elecom-exg-pro/elecom-helper)、入力デコーダーの独立した追加は [protocol fork](https://github.com/Aoki-lab1110/mouse-protocol/tree/feat/elecom-exg-pro/src/elecom) にあります。メーカー配布のバイナリやJavaScriptは取り込みません。
