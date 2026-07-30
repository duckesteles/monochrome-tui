# monochrome-tui

A terminal client for [monochrome.tf](https://monochrome.tf). Sign in with the
account you already use on the web and listen from your terminal.

## Install

Needs Rust 1.85 or newer and `libasound2` (ALSA), which every desktop
distribution already has.

```sh
git clone https://github.com/duckesteles/monochrome-tui
cd monochrome-tui
cargo build --release
./target/release/monochrome
```

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

## Licence

MIT. Not affiliated with Monochrome, Tidal, Amazon or Deezer.
