# rustotron-rn-demo

A minimal Expo RN app that boots reactotron-react-native's networking plugin
and fires a handful of HTTP requests. Used as the reference scaffold for
testing rustotron against a real RN client.

## Quick start

```bash
# From the repo root
cd examples/rn-app
npm install
# In a separate terminal, start rustotron (default port 9090):
#   cargo run --release
# Then:
npx expo start --ios      # or --android, or press w for web
```

Tap **"Fire all 6"** in the app. Rustotron's list pane should immediately
show six rows (`GET /get`, 201, 301, 404, 500, and the 1s-delayed
200).

## What you wire into your own app

Two lines you'll add to any RN project:

1. `npm install reactotron-react-native`
2. Create `src/ReactotronConfig.ts` with the exact contents from this demo
   and import it once, near the top of your app entry point (`App.tsx` /
   `index.tsx`), before any component code.

That's it. No code changes to any fetch / axios / XHR call — the
XHRInterceptor wires in transparently.

## Port collision

Rustotron and upstream Reactotron both default to port **9090** — that's
intentional, so any RN app with a default-configured Reactotron client
connects to rustotron without any code changes. You can only run one of
them at a time on the default port.

To run both side-by-side, start one of them with `--port 9091` and point
this app at that port instead (edit `RUSTOTRON_PORT` in
`src/ReactotronConfig.ts`).

## Native builds

Expo managed workflow is enough for this demo. If you already have a bare
React Native project, the setup is the same:

```ts
// Same Reactotron.configure({ name, host, port }).use(networking()).connect()
```

## Troubleshooting

- **Rustotron shows no rows after you tap a button.** The app couldn't reach
  the WS server. If you're on a real device instead of a simulator, change
  `host: '127.0.0.1'` to your Mac's LAN IP (e.g. `192.168.1.42`) and
  restart `rustotron` with `--host 0.0.0.0`.
- **App crashes on boot.** Check `metro` logs for a import ordering error —
  `ReactotronConfig` must be imported before any component that uses fetch.
- **Requests show up but bodies are `"~~~ skipped ~~~"`.** That's the
  networking plugin's default for `image/*` content types — expected.
