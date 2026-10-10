import { copyFileSync, mkdirSync, readFileSync } from "node:fs";

/**
 * Vite plugin shared by the Windows and Android hosts. The 7.5 MB catalogue
 * stays outside the startup chunk and is fetched only when Demo Device opens.
 */
export function qcDemoDeviceAssets({ parameterScalesSource, parameterDefaultsSource, referenceOutput }) {
  const scalesPath = "/reference/qc-live-parameter-scales.json";
  const defaultsPath = "/reference/qc-live-parameter-defaults.json";
  return {
    name: "qc-remote-demo-device-assets",
    configureServer(server) {
      server.middlewares.use(scalesPath, (_request, response) => {
        response.setHeader("Content-Type", "application/json");
        response.end(readFileSync(parameterScalesSource));
      });
      server.middlewares.use(defaultsPath, (_request, response) => {
        response.setHeader("Content-Type", "application/json");
        response.end(readFileSync(parameterDefaultsSource));
      });
    },
    closeBundle() {
      mkdirSync(referenceOutput, { recursive: true });
      copyFileSync(parameterScalesSource, `${referenceOutput}/qc-live-parameter-scales.json`);
      copyFileSync(parameterDefaultsSource, `${referenceOutput}/qc-live-parameter-defaults.json`);
    }
  };
}
