#ifndef QC_IOS_DRIVERKIT_CLIENT_H
#define QC_IOS_DRIVERKIT_CLIENT_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

int32_t qc_driver_service_available(void);
int32_t qc_driver_open(uint32_t *connection);
int32_t qc_driver_close(uint32_t connection);
int32_t qc_driver_call(uint32_t connection, uint32_t selector,
                       const void *input, uint32_t input_count,
                       void *output, uint32_t *output_count);

#ifdef __cplusplus
}
#endif

#endif
