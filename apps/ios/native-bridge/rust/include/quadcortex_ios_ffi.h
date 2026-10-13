#ifndef QUADCORTEX_IOS_FFI_H
#define QUADCORTEX_IOS_FFI_H

#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>

typedef struct QcIosSession QcIosSession;

QcIosSession *qc_ios_session_create(void);
void qc_ios_session_destroy(QcIosSession *session);
uint64_t qc_ios_session_opened(QcIosSession *session, uint64_t now_ms);
uint64_t qc_ios_report_epoch(QcIosSession *session, uint64_t connection);
ptrdiff_t qc_ios_push_report(QcIosSession *session, uint64_t connection,
                             uint64_t epoch, const uint8_t *report,
                             size_t report_len, uint64_t received_at_ms,
                             char *output, size_t output_capacity);
ptrdiff_t qc_ios_next_handshake(QcIosSession *session, uint64_t now_ms,
                                const char *session_id, char *output,
                                size_t output_capacity);
ptrdiff_t qc_ios_next_lifecycle_batch(QcIosSession *session, uint64_t now_ms,
                                      uint64_t unix_time_ms, char *output,
                                      size_t output_capacity);
ptrdiff_t qc_ios_lifecycle_dispatched(QcIosSession *session,
                                      const char *ticket_json, bool accepted,
                                      uint64_t now_ms, char *output,
                                      size_t output_capacity);
ptrdiff_t qc_ios_plan_gateway_write(QcIosSession *session, const char *method,
                                    const char *params_json,
                                    uint64_t started_at_ms, char *output,
                                    size_t output_capacity);
ptrdiff_t qc_ios_begin_gateway_verification(QcIosSession *session,
                                           const char *context_json,
                                           const char *verification_json,
                                           const char *method,
                                           uint64_t after_sequence,
                                           uint64_t started_at_ms,
                                           char *output,
                                           size_t output_capacity);
ptrdiff_t qc_ios_advance_gateway_verification(QcIosSession *session,
                                              uint64_t verification_id,
                                              uint64_t observation_sequence,
                                              uint64_t now_ms,
                                              const char *method,
                                              const char *verification_json,
                                              char *output,
                                              size_t output_capacity);
ptrdiff_t qc_ios_plan_gateway_read(QcIosSession *session, const char *method,
                                   const char *params_json,
                                   uint64_t started_at_ms, char *output,
                                   size_t output_capacity);
ptrdiff_t qc_ios_observe_gateway_read(const char *projection_json,
                                      uint16_t expected_type,
                                      uint16_t actual_type,
                                      const uint8_t *payload,
                                      size_t payload_len,
                                      uint64_t started_at_ms,
                                      uint64_t timeout_ms,
                                      uint64_t ingress_first_ms,
                                      uint64_t ingress_last_ms,
                                      uint64_t now_ms, char *output,
                                      size_t output_capacity);
ptrdiff_t qc_ios_compose_global_tempo_settings(const char *global_json,
                                               const char *preset_json,
                                               char *output,
                                               size_t output_capacity);
ptrdiff_t qc_ios_prepare_delivery(QcIosSession *session,
                                  const char *dispatch_json,
                                  uint16_t message_type, size_t report_count,
                                  uint64_t now_ms, char *output,
                                  size_t output_capacity);
ptrdiff_t qc_ios_delivery_admitted(QcIosSession *session,
                                   const char *ticket_json, char *output,
                                   size_t output_capacity);
ptrdiff_t qc_ios_begin_report(QcIosSession *session, const char *ticket_json,
                              size_t index, uint64_t now_ms, char *output,
                              size_t output_capacity);
ptrdiff_t qc_ios_finish_report(QcIosSession *session, const char *ticket_json,
                               size_t index, uint8_t outcome, uint64_t now_ms,
                               char *output, size_t output_capacity);
ptrdiff_t qc_ios_stop_delivery(QcIosSession *session, const char *ticket_json,
                               uint8_t reason, uint64_t now_ms, char *output,
                               size_t output_capacity);
ptrdiff_t qc_ios_operation_delivery_complete(QcIosSession *session,
                                             const char *operation_json,
                                             size_t frame_count, char *output,
                                             size_t output_capacity);
ptrdiff_t qc_ios_encode_message(uint16_t message_type, const uint8_t *payload,
                                size_t payload_len, uint8_t *output,
                                size_t output_capacity);
size_t qc_ios_report_size(void);
ptrdiff_t qc_ios_transport_status(const QcIosSession *session, char *output,
                                  size_t output_capacity);
ptrdiff_t qc_ios_current_preset(const QcIosSession *session, char *output,
                                size_t output_capacity);
ptrdiff_t qc_ios_build_control(const char *action, char *output,
                               size_t output_capacity);

#endif
