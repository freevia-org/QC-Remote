#include "QCIosDriverKitClient.h"

#include <IOKit/IOKitLib.h>
#include <mach/mach.h>

namespace {
constexpr char kDriverServiceName[] = "QCRemoteUSBDriver";

io_service_t findDriverService() {
    CFMutableDictionaryRef matching = IOServiceNameMatching(kDriverServiceName);
    if (!matching) return IO_OBJECT_NULL;
    return IOServiceGetMatchingService(kIOMainPortDefault, matching);
}
}

extern "C" int32_t qc_driver_service_available(void) {
    io_service_t service = findDriverService();
    if (service == IO_OBJECT_NULL) return 0;
    IOObjectRelease(service);
    return 1;
}

extern "C" int32_t qc_driver_open(uint32_t *connection) {
    if (!connection) return kIOReturnBadArgument;
    *connection = IO_OBJECT_NULL;
    io_service_t service = findDriverService();
    if (service == IO_OBJECT_NULL) return kIOReturnNotFound;
    kern_return_t status = IOServiceOpen(service, mach_task_self_, 0, connection);
    IOObjectRelease(service);
    return status;
}

extern "C" int32_t qc_driver_close(uint32_t connection) {
    return connection == IO_OBJECT_NULL ? kIOReturnSuccess : IOServiceClose(connection);
}

extern "C" int32_t qc_driver_call(uint32_t connection, uint32_t selector,
                                    const void *input, uint32_t input_count,
                                    void *output, uint32_t *output_count) {
    if (connection == IO_OBJECT_NULL || !output || !output_count) return kIOReturnBadArgument;
    size_t output_size = *output_count;
    kern_return_t status = IOConnectCallStructMethod(connection, selector, input, input_count,
                                                      output, &output_size);
    if (output_size > UINT32_MAX) return kIOReturnOverrun;
    *output_count = static_cast<uint32_t>(output_size);
    return status;
}
