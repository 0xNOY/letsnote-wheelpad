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
detect_area_aspect_ratio = 1.0 # Y/X の座標密度比
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
| `scroll.detect_area_aspect_ratio` | `1.0` | > 0 | Y方向の距離に掛ける補正値。円形パッドの座標密度が縦横で異なる場合は `Xの範囲 / Yの範囲` を指定。 |
| `scroll.horizontal_start` | `2` | 0..15 | π/8 単位。45°→135° のデフォルトはパッド下端。 |
| `scroll.horizontal_end` | `6` | 0..15 | |

### ログを見る

```sh
journalctl --user -u letsnote-wheelpad -f
```

スクロール感度がおかしいときは、設定ファイルの `scroll.sensitivity`（-2..+2）で調整してください。本デーモンは自動キャリブレーションを行いません — 履歴容量は Windows 完全互換のため 20 スロット固定です（DECISIONS.md D-021-followup 参照）。

## 既知の制限・非対応事項

- **`WheelUnderCursor` は設定不可。** Wayland ではコンポジタがフォーカス先サーフェスにイベントを配るため、ユーザランドからの上書きはできません。
- **テスト対象は Synaptics TM3562-3 系列のみ。** 他のタッチパッドでも `device_name_regex` を変更すれば動く可能性はありますが、動作保証はしません。
- **Excel 用矢印キーフォールバックは削除。** 現代の Excel は横ホイールイベントをネイティブで処理するため、Windows 版のハックは不要です。
- **コースティング/慣性スクロールなし。** Windows 版 WheelPad に合わせています。xf86 にはありますが、本プロジェクトでは実装しません。

## 仕組み（一段落版）

起動時に物理タッチパッドを `EVIOCGRAB` で恒久的に占有し、libinput がアタッチする 2 つの仮想 `uinput` デバイス（物理パッド能力をミラーリングしたタッチパッドと、ホイール）を生成します。物理タッチイベントはすべて仮想タッチパッドへそのまま転送するため、カーソル・タップ・クリック・マルチフィンガージェスチャは従来どおり動作します。6 状態の FSM（`Idle → Contact → Moving → Scrolling → Debounce`）が、指が外周で円を描いていると判定すると、そのジェスチャ期間中だけ転送を**抑止**します（カーソル凍結）。同時に、隣接サンプル間の方向ベクトル角を積分し、±π を超えるたびに仮想ホイールから 1 ノッチを発行します。指を離すと、位置情報を除去したリフトイベントを転送し、libinput が合成的なカーソルジャンプを起こさずジェスチャ終了を認識できるようにします。

アルゴリズムの詳細とアーキテクチャの変更経緯は `DECISIONS.md` を参照してください（パススルー化の決定は D-022、アルゴリズム選択は D-008〜D-021）。

## ライセンス

MIT。[LICENSE](LICENSE) を参照。

## 謝辞

- Panasonic — オリジナルの WheelPad 設計者。
- X.Org `xf86-input-synaptics` プロジェクト — リバースエンジニアリング時の比較対象となった「中心からの角度」リファレンス実装。
- Peter Hutterer — [2015 年の libinput 議論](https://gitlab.freedesktop.org/libinput/libinput/-/issues/)。これが libinput パッチではなくデーモンとして実装すべき理由を明らかにしてくれました。
