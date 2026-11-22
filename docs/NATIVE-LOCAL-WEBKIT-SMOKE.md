# Native local-shell WebKitGTK smoke

Run **Local desktop WebKitGTK smoke** manually on Ubuntu 24.04. It also runs
on a push to `master` that changes the smoke workflow or driver script, but
not on ordinary application changes. It builds and hashes the exact
checkout’s Aether desktop binary and verifies that the embedded Aether page,
not MiniBrowser or a mock runtime, is under automation.

The official [Ubuntu WebKitWebDriver](https://manpages.ubuntu.com/manpages/noble/man1/WebKitWebDriver.1.html)
listens only on a temporary loopback port. Its native
[GTK binary capability](https://github.com/WebKit/WebKit/blob/webkitgtk-2.52.6/Source/WebDriver/gtk/WebDriverServiceGtk.cpp)
receives that binary directly. Tauri's first-WebView automation permission is
enabled only in the private test launcher environment. There are no product
config changes, proxies, elevated browser privileges, or sandbox overrides.
Authenticated Xvfb and a private D-Bus session isolate the test, and the app
has an empty HOME, XDG directories and fixed shell fixture paths.

The smoke clicks the actual local shell controls, then types fixed commands
through WebDriver. Each owned local shell must show its own structured marker
and a distinct color in the actual WebKit canvas backing store. The test
checks that an ambiguous roster preselects neither shell and disallows a third
open, explicitly reconnects both views, checks isolated drafts and disabled
input/resize, and verifies that no draft command executed. It closes both
selected local shells and waits for each distinct recorded direct-child
identity to disappear. WebDriver closes the session; the launcher also owns
bounded process-group cleanup.

This scope is real Linux WebKitGTK/X11 under Xvfb, not a physical pointing
device, a compositor screenshot comparison, IME or Wayland coverage. The
normal mock-window PTY pressure regression separately covers detached output
beyond kernel capacity. No SSH host or user shell profile is used.
