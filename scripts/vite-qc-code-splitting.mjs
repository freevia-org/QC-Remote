/**
 * Shared production chunk policy for both native web shells.
 *
 * Framework dependencies and large immutable registries get stable cacheable
 * chunks. The rest of the application graph stays under Rolldown's automatic
 * splitting, including qc-ui's explicit lazy screen-fixture boundary.
 */
export const qcWebBuild = {
  rolldownOptions: {
    output: {
      codeSplitting: {
        groups: [
          {
            name: "vendor",
            test: /node_modules[\\/]/,
            priority: 3
          },
          {
            name: "qc-contracts",
            test: /[\\/](generated-actions|generated-gateway-methods)\.(ts|js)$/,
            priority: 2
          },
          {
            name: "qc-iconography",
            test: /[\\/](screen-icon-vectors|theme-icons|theme-icon-geometry|local-app-logos|device-category-glyph)\.(ts|tsx|js)$/,
            priority: 1
          }
        ]
      }
    }
  }
};
