package org.freevia.qcremote;

/** One mutation classification for UI target admission and write quarantine. */
final class QcGatewayTargetPolicy {
    static boolean isMutationDispatch(String dispatch) {
        return "SET_DEVICE_NAME".equals(dispatch) || "TAP_SCREEN".equals(dispatch)
            || "PREVIEW_PARAMETER".equals(dispatch) || "PLANNED_WRITE".equals(dispatch)
            || "PRESET_WRITE".equals(dispatch) || "PERSISTENT_WRITE".equals(dispatch)
            || "BACKUP".equals(dispatch);
    }

    static void requireAccepted(String method, boolean managedUi, boolean hasExpectedTarget) {
        boolean changesTarget = "device.resetSession".equals(method) || "device.disconnect".equals(method);
        if (managedUi && !hasExpectedTarget && (changesTarget || isMutationDispatch(GeneratedGatewayMethods.dispatchKind(method))))
            throw new RelayException("TARGET_SUPERSEDED", "Adopt the current physical QC target before changing it.");
    }
}
