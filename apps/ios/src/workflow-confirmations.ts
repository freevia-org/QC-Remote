let ask: ((message: string) => Promise<boolean>) | undefined;

export function registerWorkflowConfirmation(handler: (message: string) => Promise<boolean>): () => void {
  ask = handler;
  return () => { if (ask === handler) ask = undefined; };
}

export function confirmIosWorkflow(message: string): Promise<boolean> {
  return ask ? ask(message) : Promise.resolve(false);
}
