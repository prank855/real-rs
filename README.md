# <img src="res/logo.png" alt="real-rs logo" width="40"> real-rs

Reduces audio delay on Windows. Good for games, music production, and anything else where you want sound to respond instantly.

## How to use

1. Run `real-rs.exe`.
2. That's it. real-rs automatically works on all your speakers, headphones, and microphones.

Each device shows a colored dot:

- 🟢 **Green**: delay reduction is on
- 🔴 **Red**: it's off, usually because that device or its driver doesn't support it

To pause or resume a device, select it and press **Enter**, or double-click it.

If real-rs is minimized to the tray, right-click its icon to open the window or quit.

## Start automatically

Press **i** while the window is open to make real-rs launch with Windows and run in the background.

## Build it yourself

You'll need Rust 1.80 or newer with the MSVC toolchain.

```
cargo build --release
```

The program will be at `target\release\real-rs.exe`.

## Credits

A Rust rewrite of [REAL](https://github.com/miniant-git/REAL) by mini)(ant (MIT license, © 2018 mini)(ant). Not affiliated with or endorsed by the original project. See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for the upstream license notice.

## License

MIT. See [LICENSE](LICENSE).