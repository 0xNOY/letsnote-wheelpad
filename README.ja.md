# letsnote-wheelpad

> English: see [README.md](README.md).

Panasonic Let's Note の **ホイールパッド**（タッチパッド外周をなぞる円形スクロール）を Linux で再現するユーザランドデーモンです。Windows と同じく、タッチパッドの外周をゆっくり円を描くようにスワイプすると縦スクロールします。

物理タッチパッドの evdev イベントを直接読み取り、`uinput` 仮想デバイスからホイールイベントを発行するため、Wayland でも X11 でも動作します。カーソル制御は引き続き物理タッチパッドが担当し、本デーモンはスクロールイベントだけを追加します。

## なぜこれが必要か

`libinput` は Wayland 時代に円形スクロールの追加を見送りました（2015 年 Peter Hutterer の議論を参照）。したがって、Let's Note の円形スクロールを Linux で動かす唯一の方法は、evdev を介してタッチパッドを直接読み、別の仮想デバイスからホイールイベントを発行するユーザランドデーモンを実装することです。本プロジェクトはまさにそれです。

## インストール

### Ubuntu / Debian

```sh
sudo dpkg -i letsnote-wheelpad_0.1.0_amd64.deb
systemctl --user enable --now letsnote-wheelpad.service
```

### Fedora / RHEL

```sh
sudo rpm -i letsnote-wheelpad-0.1.0-1.x86_64.rpm
systemctl --user enable --now letsnote-wheelpad.service
```

### Arch

```sh
yay -S letsnote-wheelpad      # AUR
systemctl --user enable --now letsnote-wheelpad.service
```

### ソースから

```sh
git clone https://github.com/Nerahikada/letsnote-wheelpad
cd letsnote-wheelpad
cargo build --release
sudo install -Dm755 target/release/letsnote-wheelpad /usr/bin/letsnote-wheelpad
sudo install -Dm644 packaging/udev/70-letsnote-wheelpad.rules /etc/udev/rules.d/70-letsnote-wheelpad.rules
sudo install -Dm644 packaging/systemd/letsnote-wheelpad.service /etc/systemd/user/letsnote-wheelpad.service
sudo install -Dm644 packaging/modules-load/letsnote-wheelpad.conf /etc/modules-load.d/letsnote-wheelpad.conf
sudo udevadm control --reload-rules && sudo udevadm trigger
sudo modprobe uinput
systemctl --user daemon-reload
systemctl --user enable --now letsnote-wheelpad.service
```

## 設定

設定ファイルは `~/.config/letsnote-wheelpad/config.toml` です。すべてのキーは省略可能で、デフォルト値は Windows の出荷時設定と一致します。

```toml
# 通常は名前正規表現で自動検出される。手動指定は非標準のパッドのみ。
# device = "/dev/input/event4"
# device_name_regex = "Synaptics.*TM3562"

[scroll]
enable               = true   # マスター有効
reverse_vertical     = false  # 縦スクロール方向を反転
horizontal_enable    = false  # 下端ウェッジでの横スクロールを有効化
reverse_horizontal   = false
sensitivity          = 0      # -2..+2 ; 小さいほど低感度
detect_area_width    = 0      # 0..10 ; 0=外周のみ, 10=全面
detect_area_radius   = 200.0  # width=0 時の内半径（X軸の座標単位）
coordinate_y_scale   = 1.0    # Yに掛ける倍率。X範囲 / Y範囲を指定
minimum_rotation_radius = 250.0 # 小さな円運動を除外。0で無効
horizontal_start     = 2      # 円弧開始位置 (π/8 単位 ; 2 → 45°)
horizontal_end       = 6      # 円弧終了位置 (π/8 単位 ; 6 → 135°)

[log]
level = "info"  # trace | debug | info | warn | error
```

| キー | デフォルト | 範囲 | 備考 |
| --- | --- | --- | --- |
| `scroll.enable` | `true` | bool | 無効化するとデーモンは起動したまま全スクロールを抑制。 |
| `scroll.reverse_vertical` | `false` | bool | "ナチュラルスクロール" は `true`。 |
| `scroll.horizontal_enable` | `false` | bool | Windows と同じく出荷時 OFF。 |
| `scroll.reverse_horizontal` | `false` | bool | |
| `scroll.sensitivity` | `0` | -2..+2 | 倍率テーブル `[10, 14, 20, 28, 40]` のインデックス。 |
| `scroll.detect_area_width` | `0` | 0..10 | `0`=外周のみ、`10`=全面でスクロール開始可能。 |
| `scroll.detect_area_radius` | `200.0` | > 0 | `width=0` 時の内側デッドゾーン半径（X軸の生座標単位）。反応領域が広すぎる場合は大きくする。 |
| `scroll.coordinate_y_scale` | `1.0` | > 0 | すべてのY方向距離に掛ける補正値。円形パッドの座標密度が縦横で異なる場合は `Xの範囲 / Yの範囲` を指定。 |
| `scroll.minimum_rotation_radius` | `250.0` | ≥ 0 | 補正後のX軸座標単位で指定する局所的な円の最小半径。これより小さい円運動を無視し、`0`で無効。 |
| `scroll.horizontal_start` | `2` | 0..15 | π/8 単位。45°→135° のデフォルトはパッド下端。 |
| `scroll.horizontal_end` | `6` | 0..15 | |

### CF-SZ6（SYN0502）

1台のCF-SZ6で、円形パッドが `SynPS/2 Synaptics TouchPad`、
X=1210..5780、Y=1250..4680として報告されることを確認しました。
次の設定で縦方向の円スクロール動作を実機確認済みです。

```toml
device_name_regex = "SynPS/2 Synaptics TouchPad"

[scroll]
detect_area_width  = 0
detect_area_radius = 1965.0
coordinate_y_scale = 1.33236 # (5780 - 1210) / (4680 - 1250)
minimum_rotation_radius = 500.0
```

### ログを見る

```sh
journalctl --user -u letsnote-wheelpad -f
```

スクロール感度がおかしいときは、設定ファイルの `scroll.sensitivity`（-2..+2）で調整してください。本デーモンは自動キャリブレーションを行いません — detector 履歴は Windows 完全互換のため 20 sample 固定です。

## 既知の制限・非対応事項

- **`WheelUnderCursor` は設定不可。** Wayland ではコンポジタがフォーカス先サーフェスにイベントを配るため、ユーザランドからの上書きはできません。
- **縦方向の円スクロールは Synaptics TM3562-3 と CF-SZ6 SYN0502 で実機検証済み。** 他のタッチパッドでも `device_name_regex` と座標補正を設定すれば動く可能性はありますが、動作保証はしません。
- **入力プロキシの復旧処理はまだ実機検証していません。** contactまたはbuttonがactiveな起動時の待機・grab retry、signal shutdown、`SYN_DROPPED` 復旧、ボタンを押したままのスクロールは、物理タッチパッド・uinput・libinput を通した end-to-end 試験が必要です。
- **Excel 用矢印キーフォールバックは削除。** 現代の Excel は横ホイールイベントをネイティブで処理するため、Windows 版のハックは不要です。
- **コースティング/慣性スクロールなし。** Windows 版 WheelPad に合わせています。xf86 にはありますが、本プロジェクトでは実装しません。

## 入力プロキシの安全性と復旧

入力元として受け付けるのは、`ABS_MT_SLOT`、`ABS_MT_TRACKING_ID`、`ABS_MT_POSITION_X/Y`、`BTN_TOUCH` を公開する物理 Type B multitouch device だけです。自動探索では、未対応候補を除外してから同名候補の曖昧性を判定します。再帰的に自分の uinput 出力を選ばないよう、すべての `BUS_VIRTUAL` device に加え、letsnote-wheelpad 固有の名前または input ID を持つ device を拒否します。

起動時には、物理deviceをungrabbedのまま仮想deviceを作成します。MT tracking ID、`BTN_TOUCH`、物理touchpad buttonのいずれかがactiveなら、既存consumerがそのlifecycleを完了できるよう待機します。quiescent snapshotを確認してからgrabし、直ちにgrab前eventをdrainしてstateを再照会します。race windowでinputがactiveになっていればungrabして再試行します。確認済みのquiescent snapshotからgesture/routing stateを初期化し、その後にだけsystemdへ`READY=1`を送ります。恒久grab前からactiveだったcontactやbuttonをproxyへ途中から引き継ぐことはしません。

evdev streamはraw modeで読み、`SYN_DROPPED`を検出したら全slotのtracking IDと位置を再取得します。再構築streamは、変化したまたはactiveな各slotを選択し、必要なtracking終了/開始eventを出し、identityが変わっていないactive slotを含めて最新X/Yを出し、最後に物理deviceのrefreshed current slotを再選択して`SYN_REPORT`を出します。Scrolling中の捕捉contact X/Yはroutingで抑止できますが、非捕捉contactの再構築位置は転送します。ただし、`SYN_DROPPED`後にすべての補助key/ABS stateを仮想device上へ完全再構築する処理は未実装です。

以前の無条件な 5 秒 scrolling watchdog は削除しました。Scrolling 中だけ poll loop が 1 秒ごとに物理 tracking ID を確認します。捕捉中の `(slot, tracking_id)` が存在する限り、指が静止していても session を維持します。その identity が消えた場合は、同期した lift を通常の FSM へ渡して session を終了します。同期 I/O が失敗した場合は fatal error とし、grab を解放して非ゼロ終了します。

SIGTERM と SIGINT は block したうえで `signalfd` から受信し、evdev FD と同時に poll するため、stop flag の確認と blocking poll の間で signal を取りこぼしません。fatal な poll、evdev、同期、uinput error では daemon を終了し、正常終了・異常終了のどちらでも RAII guard が `EVIOCGRAB` を解放します。多重起動防止の範囲は、同一 UID・同一 XDG runtime directory 内の同一 device です。

## 仕組み（一段落版）

daemon は実行中だけ物理タッチパッドを排他的に占有し、仮想タッチパッド mirror と仮想 wheel を生成します。WheelPad session 外では event を順序どおり転送します。既存の FSM と円運動 detector が engagement した後は、捕捉 contact の `ABS_MT_POSITION_X/Y` と primary cursor mirror の `ABS_X/Y` だけを抑止します。button、slot/tracking lifecycle、補助 touch data、MSC event、非捕捉 contact は引き続き転送します。detector の数式、開始閾値、定数、20 sample 固定履歴、感度 table、停止・再開操作は変更していません。既存の accumulator が ±π を超えるたびに wheel 1 notch を発行し、捕捉した tracking ID の lift で session を終了しつつ、仮想タッチパッドへ contact lifecycle を維持します。

アルゴリズムとアーキテクチャの詳細は、source と synthetic regression test を参照してください。

## ライセンス

MIT。[LICENSE](LICENSE) を参照。

## 謝辞

- Panasonic — オリジナルの WheelPad 設計者。
- X.Org `xf86-input-synaptics` プロジェクト — リバースエンジニアリング時の比較対象となった「中心からの角度」リファレンス実装。
- Peter Hutterer — [2015 年の libinput 議論](https://gitlab.freedesktop.org/libinput/libinput/-/issues/)。これが libinput パッチではなくデーモンとして実装すべき理由を明らかにしてくれました。
