import {
  ChromiumCaptureRuntime,
  type ChromiumRuntimeApi,
} from "./chromium-runtime.js";

declare const chrome: ChromiumRuntimeApi;

new ChromiumCaptureRuntime(chrome, (error) => {
  console.error("Download Manager extension runtime error", error);
});
