/**
 * Reactotron client configuration for the rustotron demo.
 *
 * Every rustotron-compatible RN app needs two things:
 *   1. `reactotron-react-native` configured with the `networking()` plugin.
 *   2. The server host pointing at where rustotron is listening
 *      (127.0.0.1:9091 by default).
 *
 * The call to `.connect()` starts the WebSocket as soon as the app boots;
 * rustotron should be running before you launch the app, or the client
 * will retry on its own schedule.
 */

import Reactotron, { networking } from 'reactotron-react-native';

// If Reactotron can't reach your Mac:
//   - iOS Simulator / Android Emulator on this Mac: `127.0.0.1` works.
//   - Physical device / Expo Go over Wi-Fi: set `RUSTOTRON_HOST` to
//     your Mac's LAN IP (find it with `ipconfig getifaddr en0`) AND
//     start rustotron with `--host 0.0.0.0`.
const RUSTOTRON_HOST = '127.0.0.1';
const RUSTOTRON_PORT = 9091;

Reactotron
  .configure({
    name: 'rustotron-rn-demo',
    host: RUSTOTRON_HOST,
    port: RUSTOTRON_PORT,
  })
  // The networking plugin is what rustotron listens for — it emits
  // `api.response` frames for every completed XHR / fetch.
  .use(networking())
  .connect();

export default Reactotron;
