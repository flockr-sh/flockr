// Minimal stack navigation; the prototype has four screens.
export type Route =
  | { name: 'servers' }
  | { name: 'pair'; initialUrl?: string }
  | { name: 'agents'; serverId: string }
  | { name: 'terminal'; serverId: string; paneId: string; title: string };

export interface Nav {
  push(route: Route): void;
  pop(): void;
  reset(routes: Route[]): void;
}
