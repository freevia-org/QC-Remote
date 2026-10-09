import type { Plugin } from "vite";

export interface QcDemoDeviceAssetOptions {
  parameterScalesSource: string;
  parameterDefaultsSource: string;
  referenceOutput: string;
}

export function qcDemoDeviceAssets(options: QcDemoDeviceAssetOptions): Plugin;
