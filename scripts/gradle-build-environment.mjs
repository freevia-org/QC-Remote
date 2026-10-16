// Scope this policy to noninteractive build children, never a machine-wide stop.
export const BUILD_GRADLE_OPTIONS = Object.freeze([
  '-Dorg.gradle.daemon=false',
  '-Dorg.gradle.project.kotlin.compiler.execution.strategy=in-process',
]);
export function buildGradleOptions(existing = '') {
  const kept = String(existing ?? '').split(/\s+/).filter(Boolean)
    .filter(token => !/^-D(?:org\.gradle\.daemon|org\.gradle\.project\.kotlin\.compiler\.execution\.strategy)=/.test(token));
  return [...kept, ...BUILD_GRADLE_OPTIONS].join(' ');
}
export function buildGradleEnvironment(environment = process.env) {
  const entries = Object.entries(environment).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0);
  const existing = entries.find(([key]) => key.toUpperCase() === 'GRADLE_OPTS')?.[1];
  return { ...Object.fromEntries(entries.filter(([key]) => key.toUpperCase() !== 'GRADLE_OPTS')),
    GRADLE_OPTS: buildGradleOptions(existing) };
}
