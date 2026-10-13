import { useCallback, useState } from "react";

/** Suspension and bulk progress are transient publications owned by one USB session. */
export function useAndroidSessionBlocks() {
  const [deviceSuspended, setDeviceSuspended] = useState(false);
  const [deviceBulkBlocking, setDeviceBulkBlocking] = useState(false);
  const resetSessionBlocks = useCallback(() => {
    setDeviceSuspended(false);
    setDeviceBulkBlocking(false);
  }, []);
  return { deviceSuspended, deviceBulkBlocking, setDeviceSuspended, setDeviceBulkBlocking, resetSessionBlocks };
}
