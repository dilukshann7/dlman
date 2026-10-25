import type { WebMediaInspection } from './web-media.mjs';
export class AutomaticInspectionQueue {
  constructor(options: { poll(id: string): Promise<WebMediaInspection>; now?: () => number });
  request(task: { key: string; start(): Promise<WebMediaInspection>; isCurrent(): boolean }, retry?: boolean): Promise<WebMediaInspection>;
  clear(): void;
}
