/**
 * rustotron RN demo app — a single button that fires a handful of HTTP
 * requests so the dev can watch them flow through rustotron.
 *
 * The import order matters: `ReactotronConfig` must be imported BEFORE
 * any networking happens, so its XHRInterceptor is installed first.
 */

import './src/ReactotronConfig';

import { StatusBar } from 'expo-status-bar';
import { useState } from 'react';
import { Button, StyleSheet, Text, View } from 'react-native';

const ENDPOINTS = [
  'https://httpbin.org/get',
  'https://httpbin.org/status/201',
  'https://httpbin.org/status/301',
  'https://httpbin.org/status/404',
  'https://httpbin.org/status/500',
  'https://httpbin.org/delay/1',
];

export default function App() {
  const [lastStatus, setLastStatus] = useState<string | null>(null);

  const fireOne = async (url: string) => {
    try {
      const res = await fetch(url);
      setLastStatus(`${url} → ${res.status}`);
    } catch (err) {
      setLastStatus(`${url} → error: ${String(err)}`);
    }
  };

  const fireAll = async () => {
    for (const url of ENDPOINTS) {
      // Fire sequentially so the rustotron timeline is readable.
      await fireOne(url);
    }
  };

  return (
    <View style={styles.container}>
      <Text style={styles.h1}>rustotron demo</Text>
      <Text style={styles.help}>
        Make sure `rustotron` is running on 127.0.0.1:9090, then tap a button.
      </Text>

      <View style={styles.btns}>
        <Button title="Fire all 6" onPress={fireAll} />
        <Button title="GET /get" onPress={() => fireOne(ENDPOINTS[0])} />
        <Button title="GET /status/404" onPress={() => fireOne(ENDPOINTS[3])} />
        <Button title="GET /status/500" onPress={() => fireOne(ENDPOINTS[4])} />
      </View>

      <Text style={styles.status}>{lastStatus ?? 'no requests yet'}</Text>
      <StatusBar style="auto" />
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: '#fff',
    alignItems: 'center',
    justifyContent: 'center',
    padding: 16,
    gap: 12,
  },
  h1: { fontSize: 22, fontWeight: '700' },
  help: { fontSize: 14, color: '#444', textAlign: 'center' },
  btns: { gap: 8, width: '100%' },
  status: { fontSize: 12, color: '#666', marginTop: 16 },
});
