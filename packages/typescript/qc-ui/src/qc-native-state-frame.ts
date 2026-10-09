import type { Dispatch, MutableRefObject, SetStateAction } from "react";
import type { NativeStateFrame, PresetSnapshot } from "@qc-remote/client";
import { synchronizeTempoPulseEpoch, type QcStateUpdate } from "@qc-remote/core";

export interface QcNativeStateFrameConsumer {
  sequence: MutableRefObject<number>;
  streamId?: MutableRefObject<string | undefined>;
  recoveryRevision?: MutableRefObject<number>;
  available?: MutableRefObject<boolean>;
  consume(states: readonly QcStateUpdate[], observedAt?: number, frame?: {sequence: number; streamId?: string}): unknown;
  setSnapshot: Dispatch<SetStateAction<PresetSnapshot>>;
  /**
   * Called when the frame stream is discontinuous: either a replacement
   * producer restarted its sequence, or a bounded native/UI queue dropped one
   * or more frames. Frames are deltas, so the host must pull a full snapshot
   * whenever either happens.
   */
  onStreamRestart?(reason: "initial" | "restarted" | "gap"): void;
}

/**
 * Apply the ordering, timestamp, and tempo-clock contract shared by every
 * native frame source. Platform adapters only subscribe to their OS event API.
 *
 * The sequence counter belongs to the producer, not to the session: a broker
 * that is replaced - after a crash, a reset, or a reconnect - starts counting
 * again from one. Treating that as "already seen" silently discarded every
 * frame from the new producer forever, freezing the UI against a device that
 * was still perfectly healthy. Within one producer the sequence only ever
 * rises. An identified producer can safely discard delayed older frames;
 * legacy producers fall back to detecting a restart from a lower sequence.
 */
export function consumeQcNativeStateFrame(
  frame: NativeStateFrame<QcStateUpdate>,
  consumer: QcNativeStateFrameConsumer
): boolean {
  const previousSequence = consumer.sequence.current;
  // The recovery poll can establish a stream/cursor before the platform's
  // pushed-event listener receives its first frame. That is still initial
  // attachment from the live listener's point of view, even if the producer
  // changed while the host was completing its authoritative connect-time
  // snapshot. Only a producer change after at least one pushed frame is a
  // runtime restart worth surfacing to the user.
  const liveStreamWasAvailable = consumer.available?.current;
  if (!Number.isSafeInteger(frame.sequence) || frame.sequence < 1) return false;
  const identified = !!consumer.streamId && typeof frame.streamId === "string" && frame.streamId.length > 0;
  const previousStream = consumer.streamId?.current;
  const restarted = identified && previousStream !== undefined
    ? previousStream !== frame.streamId
    : frame.sequence < previousSequence;
  const missedFrames = frame.sequence > previousSequence + 1;
  if (identified) consumer.streamId!.current = frame.streamId;
  if (!restarted && frame.sequence <= consumer.sequence.current) return false;
  if (consumer.available) consumer.available.current = true;
  consumer.sequence.current = frame.sequence;
  if (restarted || missedFrames) {
    if (consumer.recoveryRevision) consumer.recoveryRevision.current++;
    // A new subscriber can join a healthy producer at any sequence. It still
    // needs a complete read, but has not observed a restart or lost live data.
    consumer.onStreamRestart?.(liveStreamWasAvailable === false
      || (previousSequence === 0 && previousStream === undefined)
      ? "initial" : restarted ? "restarted" : "gap");
  }
  consumer.consume(frame.states, frame.observedAt, {sequence: frame.sequence, streamId: frame.streamId});
  if (frame.tempoClock) {
    // Only the beat-start message (tick absent or 0) anchors the lamp; tick 1
    // is a pulse flag about 23 ms later and is not a position in the beat.
    const tick = Math.max(0, frame.tempoClock.currentTick ?? 0);
    consumer.setSnapshot((current) => {
      const epoch = synchronizeTempoPulseEpoch(
        current.tempoPulseEpochMs, frame.observedAt, tick, current.tempo
      );
      return epoch === current.tempoPulseEpochMs
        ? current
        : { ...current, tempoPulseEpochMs: epoch };
    });
  }
  return true;
}
