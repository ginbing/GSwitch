// Quota reads run in independent lanes, so a refresh the user asked for never
// waits behind automatic work. Each lane runs at most its limit of tasks at
// once and starts waiting tasks in the order they arrived.
export class RefreshLanes<Lane extends string> {
  private readonly running = new Map<Lane, number>();
  private readonly waiting = new Map<Lane, Array<() => void>>();

  constructor(private readonly limits: Record<Lane, number>) {}

  async run<T>(lane: Lane, task: () => Promise<T>): Promise<T> {
    const running = this.running.get(lane) ?? 0;
    if (running < this.limits[lane]) {
      this.running.set(lane, running + 1);
    } else {
      // A finishing task hands its slot straight to the next waiter, so a
      // task arriving in between can never push the lane over its limit.
      await new Promise<void>((resolve) => {
        const queue = this.waiting.get(lane) ?? [];
        queue.push(resolve);
        this.waiting.set(lane, queue);
      });
    }
    try {
      return await task();
    } finally {
      const next = this.waiting.get(lane)?.shift();
      if (next) {
        next();
      } else {
        this.running.set(lane, (this.running.get(lane) ?? 1) - 1);
      }
    }
  }
}
