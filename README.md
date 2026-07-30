# monochrome-tui

A terminal client for [monochrome.tf](https://monochrome.tf). Sign in with the
account you already use on the web and listen from your terminal.

## Install

Needs Rust 1.85 or newer and `libasound2` (ALSA), which every desktop
distribution already ships.

```sh
git clone https://github.com/duckesteles/monochrome-tui
cd monochrome-tui
cargo install --path crates/monochrome-tui --root ~/.local
```

Then run it from anywhere:

```sh
monochrome
```

That is the whole installation: one 10 MB file in `~/.local/bin`, which is
already on the path on most systems. The build happens in a temporary
directory, so once it finishes you can delete the cloned folder.

If your shell answers `command not found`, `~/.local/bin` is not on your path.
Add it:

```sh
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc      # zsh
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc     # bash
fish_add_path ~/.local/bin                                   # fish
```

To remove it later:

```sh
cargo uninstall --root ~/.local monochrome-tui
```

If you built with `cargo build` instead, the binary is at
`target/release/monochrome` and the build leaves several gigabytes in
`target/`; `cargo clean` frees that.

## Signing in

Email and password. Your token is kept in the system keyring.

## Playing music

Search with `/`, press enter on a track. Your saved tracks, playlists and
history are the same ones the web app has, and changes sync back.

The first time you play something you will be asked for an Amazon token. This
is not optional and it is not something this client can avoid: the gateway that
serves the audio sits behind a Cloudflare check that a terminal cannot pass.

To get one:

1. Open [monochrome.tf](https://monochrome.tf) and play anything.
2. In the browser console, run
   `copy(localStorage.getItem('amazon_turnstile_jwt'))`.
3. Paste it into the terminal and press enter.

It lasts about an hour and survives restarts. If you ever get a permanent
gateway credential, put it in the config and you will never see the prompt:

```toml
[amazon]
bypass_token = "..."
```

## Keys

Press `?` in the app. The short version:

| | |
| --- | --- |
| `j` `k`, arrows | move |
| `enter` | play or open |
| `esc` | back |
| `/` | search |
| `space` | pause |
| `←` `→` | seek |
| `+` `-` | volume |
| `s` | shuffle |
| `f` | save to library |
| `Q` | quit |

## When something is wrong

```sh
monochrome --doctor              # is the account and gateway working
monochrome --play "artist song"  # does audio work, without opening the app
```

`--doctor` will tell you if your Amazon token has expired, which is the usual
cause of a track refusing to play.

## Where things are kept

```
~/.config/monochrome-tui/config.toml        settings
~/.local/state/monochrome-tui/snapshot.json your library, so the first screen is not empty
~/.local/state/monochrome-tui/credentials   only if your system has no keyring
~/.local/state/monochrome-tui/log           only with --verbose
~/.cache/monochrome-tui/                    the track being played, nothing more
```

All of it is readable only by you.

Nothing you listen to is kept. The track currently playing is buffered to a
file so seeking is instant; that file is unlinked the moment it is opened, so
it never appears in the directory, and the space is released when you move to
the next track. One track at a time, around 15 MB for a four minute lossless
song. Sign out and the library snapshot is cleared.

## Configuration

`~/.config/monochrome-tui/config.toml`, written on first run. Worth knowing:

```toml
[playback]
quality = "lossless"   # low | high | lossless | hi-res
volume  = 0.7

[ui]
accent  = ""           # a colour name or #rrggbb; empty stays monochrome
spacing = "compact"    # "roomy" puts a blank line between rows
```

The interface never sets a background colour, so it inherits your terminal
theme and transparency as-is.

## What is not here

Visualisers, themes, listening parties, equaliser, podcasts, downloads,
scrobbling, local files, lyrics, OAuth sign-in.

## Credits

Built against the services of
[monochrome-music/monochrome](https://github.com/monochrome-music/monochrome)
(Apache-2.0), which this client shares an account and a library with. None of
its code is used here; see [THIRD-PARTY.md](THIRD-PARTY.md).

## Licence

MIT. Not affiliated with Monochrome, Tidal, Amazon or Deezer.
