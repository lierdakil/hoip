# HID over IP

Share keyboard and mouse (or other HID inputs) over TCP.

## Motivation

A lot of "keyboard/mouse sharing over network" programs struggle somewhat in
Linux/Wayland context. To name a few:

- [InputLeap](https://github.com/input-leap/input-leap)
- [DeskFlow](https://github.com/deskflow/deskflow)
- [LAN-Mouse](https://github.com/feschber/lan-mouse)

All of the above capture/emulate input events using Wayland protocols, which,
depending on the compositor and whatnot, can lead to a lot of _weird quirks_.
One particular example I've encountered is _some_ modkeys work fine but _others_
fail completely, with no obvious pattern.

In case one doesn't need cross-platform compatibility and is willing to run some
programs with slightly elevated privileges, there's actually a much simpler way
to achieve the desired effect: send `/dev/input/event*` events over the network.

Since this approach works on the layer below graphical session, it sidesteps the
whole Wayland mess, plus it works just about as well for terminal sessions, too.

Aside from the need for the elevated privileges setup, one other downside is
one loses "hot edges" thing: switching between machines has to be done
explicitly via a hotkey or a mouse gesture. But personally I find "hot edges"
more annoying than useful anyway.

## How it works

Basically, there are two parts here, "server" (`hoips`) which has access to
physical keyboard/mouse, and "client" (`hoipc`) which emulates those.

Note this server/client split is purely conceptual, in practice it's the
"client" that _listens_ to network connections and the "server" that _initiates_
them (it just makes way more sense with how TCP sessions work).

"Client" basically just receives input events over the network and feeds them
verbatim to an uinput virtual devcie. "Server" is a little more involved, as it
has to decide when to hog exclusive access to input devices (basically whenever
it sends events over the network, it should prevent those same events reaching
the host), but in practice it's extremely straightforward.

## How to build

Aside from the ususal `cargo build --release`, this repo is a Nix flake, so you
can build via `nix build`. Aside from the `default` output, it also provides `static` output for static musl builds, and apps to run it ad-hoc. So in principle you can test-drive via `nix run github:lierdakil/hoip#server` and `nix run github:lierdakil/hoip#client` respectively.

## Stability

While this works mostly fine, I make no guarantees about not changing something
drastically in the future. At this point, this is little more than a tech demo
I've slapped together over a couple evenings, treat it as such.

## License

MIT, see LICENSE.
