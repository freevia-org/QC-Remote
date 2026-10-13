#include "QCRemoteUSBDriver.h"
#include "QCRemoteUSBUserClient.h"

#include <DriverKit/IOBufferMemoryDescriptor.h>
#include <USBDriverKit/USBDriverKit.h>
#include <string.h>

namespace {
constexpr uint32_t kReadReport = 0;
constexpr uint32_t kWriteReport = 1;
constexpr uint32_t kCopyDiagnostics = 2;
constexpr uint32_t kReportLength = 129;
constexpr uint8_t kHidInterfaceClass = 0x03;
constexpr uint8_t kPreferredQcHidInterface = 5;
constexpr uint16_t kMinimumQcInterruptPacketSize = 128;
constexpr uint8_t kInterruptTransfer = 0x03;
constexpr uint8_t kEndpointDirectionIn = 0x80;
constexpr uint8_t kSetReport = 0x09;
constexpr uint8_t kClassInterfaceOut = 0x21;

struct QcUsbDiagnostics {
    uint32_t version;
    uint32_t interfaceNumber;
    uint32_t inputEndpointAddress;
    uint32_t inputPacketSize;
    uint32_t reportLength;
    uint32_t reserved;
};

struct QcUsbWriteResult {
    uint32_t bytesTransferred;
    int32_t usbStatus;
};
}

kern_return_t IMPL(QCRemoteUSBDriver, Start) {
    kern_return_t status = Start(provider, SUPERDISPATCH);
    if (status != kIOReturnSuccess) return status;

    ivars->device = OSDynamicCast(IOUSBHostDevice, provider);
    if (!ivars->device) return kIOReturnBadArgument;

    status = ivars->device->Open(this, 0, 0);
    if (status != kIOReturnSuccess) return status;
    ivars->deviceOpened = true;

    kern_return_t selectedStatus = kIOReturnNotFound;
    // QC's measured control HID function is interface 5. Try it first, then
    // allow a product variant to use another HID interface only when its
    // interrupt-IN endpoint can carry the complete QC report.
    for (uint8_t pass = 0; pass < 2 && !ivars->inputPipe; ++pass) {
        uintptr_t iterator = 0;
        status = ivars->device->CreateInterfaceIterator(&iterator);
        if (status != kIOReturnSuccess) {
            ReleaseUsbResources();
            return status;
        }
        IOUSBHostInterface *candidate = nullptr;
        while (ivars->device->CopyInterface(iterator, &candidate) == kIOReturnSuccess && candidate) {
            const IOUSBConfigurationDescriptor *configuration = candidate->CopyConfigurationDescriptor();
            if (!configuration) {
                candidate->release();
                candidate = nullptr;
                continue;
            }

            const IOUSBInterfaceDescriptor *interfaceDescriptor = candidate->GetInterfaceDescriptor(configuration);
            bool selected = false;
            if (interfaceDescriptor && interfaceDescriptor->bInterfaceClass == kHidInterfaceClass) {
                const uint8_t interfaceNumber = interfaceDescriptor->bInterfaceNumber;
                const bool preferred = interfaceNumber == kPreferredQcHidInterface;
                IOUSBHostFreeDescriptor(configuration);
                if ((pass == 0 && !preferred) || (pass == 1 && preferred)) {
                    candidate->release();
                    candidate = nullptr;
                    continue;
                }
                selectedStatus = candidate->Open(this, 0, 0);
                if (selectedStatus == kIOReturnSuccess) {
                    ivars->hidInterface = candidate;
                    ivars->interfaceOpened = true;
                    ivars->interfaceNumber = interfaceNumber;

                    const IOUSBConfigurationDescriptor *activeConfiguration = candidate->CopyConfigurationDescriptor();
                    const IOUSBInterfaceDescriptor *activeDescriptor = activeConfiguration
                        ? candidate->GetInterfaceDescriptor(activeConfiguration) : nullptr;
                    const IOUSBDescriptorHeader *descriptor = reinterpret_cast<const IOUSBDescriptorHeader *>(activeDescriptor);
                    for (uint8_t endpointIndex = 0;
                         activeDescriptor && endpointIndex < activeDescriptor->bNumEndpoints;
                         ++endpointIndex) {
                        descriptor = IOUSBGetNextDescriptor(activeConfiguration, descriptor);
                        while (descriptor && descriptor->bDescriptorType != kIOUSBDescriptorTypeEndpoint) {
                            descriptor = IOUSBGetNextDescriptor(activeConfiguration, descriptor);
                        }
                        if (!descriptor) break;

                        const auto *endpoint = reinterpret_cast<const IOUSBEndpointDescriptor *>(descriptor);
                        const uint8_t transferType = endpoint->bmAttributes & 0x03;
                        if (transferType == kInterruptTransfer
                            && (endpoint->bEndpointAddress & kEndpointDirectionIn) != 0
                            && endpoint->wMaxPacketSize >= kMinimumQcInterruptPacketSize) {
                            status = candidate->CopyPipe(endpoint->bEndpointAddress, &ivars->inputPipe);
                            if (status == kIOReturnSuccess) {
                                ivars->inputEndpointAddress = endpoint->bEndpointAddress;
                                ivars->inputPacketSize = endpoint->wMaxPacketSize;
                                break;
                            }
                        }
                    }
                    if (activeConfiguration) IOUSBHostFreeDescriptor(activeConfiguration);
                    if (ivars->inputPipe) {
                        selected = true;
                        break;
                    }
                    selectedStatus = kIOReturnNotFound;
                    candidate->Close(this, 0);
                    ivars->interfaceOpened = false;
                    ivars->hidInterface = nullptr;
                }
            } else {
                IOUSBHostFreeDescriptor(configuration);
            }

            if (!selected) {
                candidate->release();
                candidate = nullptr;
            }
        }
        ivars->device->DestroyInterfaceIterator(iterator);
    }
    if (!ivars->inputPipe) {
        ReleaseUsbResources();
        return selectedStatus;
    }

    return kIOReturnSuccess;
}

kern_return_t IMPL(QCRemoteUSBDriver, Stop) {
    ReleaseUsbResources();
    return Stop(provider, SUPERDISPATCH);
}

void QCRemoteUSBDriver::ReleaseUsbResources() {
    if (ivars->inputPipe) {
        ivars->inputPipe->release();
        ivars->inputPipe = nullptr;
    }
    if (ivars->interfaceOpened && ivars->hidInterface) {
        ivars->hidInterface->Close(this, 0);
        ivars->interfaceOpened = false;
    }
    if (ivars->hidInterface) {
        ivars->hidInterface->release();
        ivars->hidInterface = nullptr;
    }
    if (ivars->deviceOpened && ivars->device) {
        ivars->device->Close(this, 0);
        ivars->deviceOpened = false;
    }
    ivars->device = nullptr;
    ivars->interfaceNumber = 0;
    ivars->inputEndpointAddress = 0;
    ivars->inputPacketSize = 0;
}

kern_return_t QCRemoteUSBDriver::ReadReport(OSData **report) {
    if (!report || !ivars->inputPipe || !ivars->device) return kIOReturnNotReady;

    IOBufferMemoryDescriptor *buffer = nullptr;
    kern_return_t status = ivars->device->CreateIOBuffer(kIOMemoryDirectionIn, kReportLength, &buffer);
    if (status != kIOReturnSuccess || !buffer) return status;

    uint32_t transferred = 0;
    status = ivars->inputPipe->IO(buffer, kReportLength, &transferred, 0);
    if (status == kIOReturnSuccess && transferred > 0 && transferred <= kReportLength) {
        IOAddressSegment segment = {};
        status = buffer->GetAddressRange(&segment);
        if (status == kIOReturnSuccess) {
            *report = OSData::withBytes(reinterpret_cast<const void *>(segment.address), transferred);
            if (!*report) status = kIOReturnNoMemory;
        }
    }
    buffer->release();
    return status;
}

kern_return_t QCRemoteUSBDriver::WriteReport(OSData *report, uint32_t *bytesTransferred, kern_return_t *usbStatus) {
    if (!report || !bytesTransferred || !usbStatus || !ivars->hidInterface || !ivars->device)
        return kIOReturnNotReady;
    if (report->getLength() != kReportLength) return kIOReturnBadArgument;
    const auto *reportBytes = static_cast<const uint8_t *>(report->getBytesNoCopy());
    if (!reportBytes || reportBytes[0] != 2) return kIOReturnBadArgument;

    IOBufferMemoryDescriptor *buffer = nullptr;
    kern_return_t status = ivars->device->CreateIOBuffer(kIOMemoryDirectionOut, kReportLength, &buffer);
    if (status != kIOReturnSuccess || !buffer) return status;

    IOAddressSegment segment = {};
    status = buffer->GetAddressRange(&segment);
    if (status == kIOReturnSuccess) {
        memcpy(reinterpret_cast<void *>(segment.address), reportBytes, kReportLength);
        uint16_t transferred = 0;
        *usbStatus = ivars->hidInterface->DeviceRequest(kClassInterfaceOut, kSetReport, 0x0202, ivars->interfaceNumber,
            kReportLength, buffer, &transferred, 1'000);
        *bytesTransferred = transferred;
        if (*usbStatus == kIOReturnSuccess) status = kIOReturnSuccess;
        else status = *usbStatus;
    }
    buffer->release();
    return status;
}

kern_return_t QCRemoteUSBDriver::CopyDiagnostics(OSData **diagnostics) {
    if (!diagnostics || !ivars->hidInterface || !ivars->inputPipe) return kIOReturnNotReady;
    const QcUsbDiagnostics value = {
        2,
        ivars->interfaceNumber,
        ivars->inputEndpointAddress,
        ivars->inputPacketSize,
        kReportLength,
        0,
    };
    return (*diagnostics = OSData::withBytes(&value, sizeof(value))) ? kIOReturnSuccess : kIOReturnNoMemory;
}

kern_return_t IMPL(QCRemoteUSBDriver, NewUserClient) {
    if (type != 0 || !userClient) return kIOReturnBadArgument;
    IOService *service = nullptr;
    kern_return_t status = Create(this, "QCRemoteUSBDriverUserClientProperties", &service);
    if (status != kIOReturnSuccess) return status;
    *userClient = OSDynamicCast(IOUserClient, service);
    if (*userClient) return kIOReturnSuccess;
    service->release();
    return kIOReturnNoMemory;
}

kern_return_t IMPL(QCRemoteUSBUserClient, Start) {
    kern_return_t status = Start(provider, SUPERDISPATCH);
    if (status != kIOReturnSuccess) return status;
    ivars->driver = OSDynamicCast(QCRemoteUSBDriver, provider);
    return ivars->driver ? kIOReturnSuccess : kIOReturnBadArgument;
}

kern_return_t IMPL(QCRemoteUSBUserClient, Stop) {
    ivars->driver = nullptr;
    return Stop(provider, SUPERDISPATCH);
}

kern_return_t IMPL(QCRemoteUSBUserClient, ExternalMethod) {
    (void)dispatch;
    (void)target;
    (void)reference;
    if (!arguments || !ivars->driver) return kIOReturnNotReady;

    switch (selector) {
        case kReadReport: {
            if (arguments->structureInput || arguments->structureInputDescriptor
                || arguments->structureOutputMaximumSize < kReportLength) return kIOReturnBadArgument;
            OSData *report = nullptr;
            kern_return_t status = ivars->driver->ReadReport(&report);
            if (status != kIOReturnSuccess) return status;
            arguments->structureOutput = report;
            return kIOReturnSuccess;
        }
        case kWriteReport: {
            if (!arguments->structureInput || arguments->structureInput->getLength() != kReportLength
                || arguments->structureOutputMaximumSize < sizeof(QcUsbWriteResult)) return kIOReturnBadArgument;
            uint32_t bytesTransferred = 0;
            kern_return_t usbStatus = kIOReturnError;
            kern_return_t status = ivars->driver->WriteReport(arguments->structureInput, &bytesTransferred, &usbStatus);
            if (status != kIOReturnSuccess) return status;
            const QcUsbWriteResult result = { bytesTransferred, usbStatus };
            arguments->structureOutput = OSData::withBytes(&result, sizeof(result));
            return arguments->structureOutput ? kIOReturnSuccess : kIOReturnNoMemory;
        }
        case kCopyDiagnostics: {
            if (arguments->structureInput || arguments->structureInputDescriptor
                || arguments->structureOutputMaximumSize < sizeof(QcUsbDiagnostics)) return kIOReturnBadArgument;
            return ivars->driver->CopyDiagnostics(&arguments->structureOutput);
        }
        default:
            return kIOReturnUnsupported;
    }
}
