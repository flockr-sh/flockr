// Starts the app. The web build has its own startup in startup.web.ts.
import { registerRootComponent } from 'expo';

export function start(): void {
  registerRootComponent(require('../App').default);
}
