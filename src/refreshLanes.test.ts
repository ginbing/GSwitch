import { describe, expect, it } from "vitest";

import { RefreshLanes } from "./refreshLanes";

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

describe("quota refresh lanes", () => {
  it("runs at most the lane limit at once and starts waiting tasks in order", async () => {
    const lanes = new RefreshLanes({ manual: 2 });
    const gates = Array.from({ length: 4 }, () => deferred());
    const started: number[] = [];
    let active = 0;
    let maximum = 0;

    const tasks = gates.map((gate, index) =>
      lanes.run("manual", async () => {
        started.push(index);
        active += 1;
        maximum = Math.max(maximum, active);
        await gate.promise;
        active -= 1;
      }),
    );

    await Promise.resolve();
    expect(started).toEqual([0, 1]);
    gates[1]!.resolve();
    await tasks[1];
    // A task arriving while a slot is being handed over still waits its turn.
    const late = lanes.run("manual", async () => {
      started.push(4);
    });
    gates[0]!.resolve();
    gates[2]!.resolve();
    gates[3]!.resolve();
    await Promise.all([...tasks, late]);

    expect(maximum).toBe(2);
    expect(started).toEqual([0, 1, 2, 3, 4]);
  });

  it("keeps lanes independent, so one lane never waits behind another", async () => {
    const lanes = new RefreshLanes({ background: 1, manual: 1 });
    const background = deferred();
    const backgroundTask = lanes.run("background", () => background.promise);

    await expect(lanes.run("manual", async () => "manual done")).resolves.toBe("manual done");

    background.resolve();
    await backgroundTask;
  });

  it("frees the slot when a task fails", async () => {
    const lanes = new RefreshLanes({ signIn: 1 });
    await expect(lanes.run("signIn", async () => { throw new Error("rejected"); })).rejects.toThrow("rejected");
    await expect(lanes.run("signIn", async () => "next")).resolves.toBe("next");
  });
});
