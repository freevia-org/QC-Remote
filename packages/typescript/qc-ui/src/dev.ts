// Development-only entry point (`@qc-remote/ui/dev`). Nothing here ships to
// users: the apps import it only behind `import.meta.env.DEV`, and it is kept
// out of the public barrel so a production consumer cannot reach it by accident.
export { CorOsProductionScreenAudit } from "./production-screen-audit";
