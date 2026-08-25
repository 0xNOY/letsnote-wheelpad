# letsnote-wheelpad

[English: README.md](README.md)

`letsnote-wheelpad` は、Panasonic Let's Note のホイールパッドによる円形スクロールを Linux で利用するためのデーモンです。物理タッチパッドの入力を仮想タッチパッドへ転送し、円形ジェスチャーを仮想ホイールから出力します。Wayland と X11 に対応し、デーモン停止時は物理デバイスを解放して通常のタッチパッド入力を復旧できる fail-open 設計です。

## インストール

デーモンと systemd、udev、sysusers、設定、移行用ファイルをまとめて導入できるパッケージの利用を推奨します。パッケージをインストールしただけでは WheelPad は有効になりません。

### Arch Linux

[AUR パッケージ](https://aur.archlinux.org/packages/letsnote-wheelpad-bin)をインストールします。

```sh
yay -S letsnote-wheelpad-bin
```

`letsnote-wheelpad-bin` は、代替パッケージの `letsnote-wheelpad` および `letsnote-wheelpad-git` と競合します。

### Debian / Ubuntu

[v0.2.0 Release](https://github.com/0xNOY/letsnote-wheelpad/releases/tag/v0.2.0) から `.deb` をダウンロードし、APT でインストールします。

```sh
sudo apt install ./letsnote-wheelpad_*_amd64.deb
```

### Fedora / RPM 系

[v0.2.0 Release](https://github.com/0xNOY/letsnote-wheelpad/releases/tag/v0.2.0) から `.rpm` をダウンロードし、DNF でインストールします。

```sh
sudo dnf install ./letsnote-wheelpad-*.x86_64.rpm
```

## クイックスタート: 推奨 system mode

システムモードでは専用の `letsnote-wheelpad` ユーザーでデーモンを実行します。ログインユーザーに raw evdev や `/dev/uinput` への直接アクセス権を与えず、再起動後も自動的に動作します。

まず対応デバイスを確認します。従来のユーザーサービスが動作している場合は、システムモードの有効化前に停止します。

```sh
pkexec /usr/libexec/letsnote-wheelpad-migrate status
systemctl --user disable --now letsnote-wheelpad.service
pkexec /usr/libexec/letsnote-wheelpad-migrate enable \
  --device /dev/input/eventN
```

`status` が表示した `/dev/input/eventN` を使ってください。イベント番号は再起動などで変わります。v0.2.0 の移行機能は、対応する物理タッチパッドがちょうど 1 台ある環境を前提とします。`status` または `enable` がログインユーザーの名前付き ACL を報告した場合は、表示された正確なノードから、そのユーザーの ACL entry だけを削除して再実行します。

```sh
uid="$(id -u)"
pkexec /usr/bin/setfacl -x "u:${uid}" /dev/input/eventN
pkexec /usr/bin/setfacl -x "u:${uid}" /dev/uinput
```

`setfacl --remove-all` は使用しないでください。移行 helper は名前付き ACL を自動削除しません。

## 設定

システムモードは `/etc/letsnote-wheelpad/config.toml` を読みます。従来のユーザーモードは `~/.config/letsnote-wheelpad/config.toml` を読みます。移行時にユーザー設定がシステム設定へ自動コピーされることはありません。

よく調整する設定は次のとおりです。

- `scroll.sensitivity`
- `scroll.reverse_vertical`
- `scroll.horizontal_enable`
- `scroll.detect_area_radius`
- `scroll.coordinate_y_scale`
- `scroll.minimum_rotation_radius`

全項目とデフォルト値はパッケージに含まれる設定ファイルを参照してください。次の値はテストに使用した 1 台の CF-SZ6 で確認したもので、すべての CF-SZ6 に共通する設定ではありません。

```toml
device_name_regex = "SynPS/2 Synaptics TouchPad"

[scroll]
detect_area_width = 0
detect_area_radius = 1965.0
coordinate_y_scale = 1.33236
minimum_rotation_radius = 500.0
```

## 状態とログ

システムモードでは次を使います。

```sh
pkexec /usr/libexec/letsnote-wheelpad-migrate status
journalctl -u 'letsnote-wheelpad@*.service' -f
```

従来のユーザーモードでは次を使います。

```sh
journalctl --user -u letsnote-wheelpad.service -f
```

## 無効化とアンインストール

パッケージを削除する前に、`status` が現在表示するイベントを使ってシステムモードを無効にします。

```sh
pkexec /usr/libexec/letsnote-wheelpad-migrate status

pkexec /usr/libexec/letsnote-wheelpad-migrate disable \
  --device /dev/input/eventN
```

従来のサービスが有効なら、そちらも停止してください。Arch は移行状態またはデーモンが残っている間、パッケージの削除を意図的に拒否します。

Arch:

```sh
yay -R letsnote-wheelpad-bin
```

Debian / Ubuntu:

```sh
sudo apt remove letsnote-wheelpad
```

Fedora / RPM 系:

```sh
sudo dnf remove letsnote-wheelpad
```

downgrade の前にもシステムモードを無効にし、すべてのパッケージデーモンを停止してください。この準備をしない Arch または RPM の直接 downgrade は非対応です。

## 従来のユーザーモード

従来のユーザーモードは、ユーザーごとの設定を使う互換用モードです。

```sh
systemctl --user enable --now letsnote-wheelpad.service
```

従来のデーモンと system デーモンを同時に実行しないでください。新規インストールではシステムモードを推奨します。

## 検証済みハードウェアと制限

v0.2.0 の最終検証環境は次のとおりです。

- Panasonic CF-SZ6
- `SynPS/2 Synaptics TouchPad` (`0011/0002/0007`)
- Wayland / Hyprland

他の Let's Note では、タッチパッド名や座標の調整が必要な場合があります。これは互換性を保証するものではありません。v0.2.0 のシステムモード移行は、対応する物理タッチパッドがちょうど 1 台ある環境を前提とします。システムモードは再起動を含めて実機検証済みです。

慣性・コースティングスクロールはありません。Wayland では compositor が入力先を決めるため、`WheelUnderCursor` を X11 と同じ方法で上書きすることもできません。

## 仕組み

デーモンは物理 evdev デバイスを読み、通常のタッチパッド入力を仮想タッチパッドへ転送します。円形ジェスチャー中は別の仮想デバイスからホイールイベントを出し、通常の pointer、contact、button 動作は維持します。異常終了時と通常停止時には物理デバイスの grab を解放し、通常入力を復旧できるようにします。

入力 proxy と復旧処理の詳細は、実装と regression test に含まれます。

## 開発用ビルド

```sh
git clone https://github.com/0xNOY/letsnote-wheelpad.git
cd letsnote-wheelpad
cargo build --release
cargo test
```

通常のインストールでは、必要な systemd、udev、sysusers、移行用ファイルをまとめて導入できる配布パッケージを使用してください。

## License と謝辞

[MIT License](LICENSE) です。

オリジナルの WheelPad を設計した Panasonic、角度計算の参考実装を提供した X.Org `xf86-input-synaptics`、user space 実装の方針を明確にした libinput の議論と Peter Hutterer に感謝します。
