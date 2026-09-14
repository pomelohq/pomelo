#ifndef POM_FSWATCH_H
#define POM_FSWATCH_H

#include <CoreServices/CoreServices.h>

FSEventStreamRef pom_fsevents_start(const char *path, int handle, dispatch_queue_t queue);
void pom_fsevents_stop(FSEventStreamRef stream);
dispatch_queue_t pom_fsevents_queue(void);
void pom_fsevents_release_queue(dispatch_queue_t queue);

#endif
