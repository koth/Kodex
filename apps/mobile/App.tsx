// App entry. The RNG polyfill MUST be imported before any @noble/* crypto so
// X25519/HKDF/ChaCha20 use a secure RNG on Hermes (no Math.random fallback).
import "react-native-get-random-values";

import { registerRootComponent } from "expo";

import { Navigation } from "./src/app/navigation";

function App() {
  return <Navigation />;
}

registerRootComponent(App);
