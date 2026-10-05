/* Minimal stand-in for the Apple SDK header, used only to type-check the
 * macOS build from Linux (scripts/macos/check.sh). Never used to link. */
#pragma once
#define TARGET_OS_MAC 1
#define TARGET_OS_OSX 1
#define TARGET_OS_IPHONE 0
#define TARGET_OS_IOS 0
#define TARGET_OS_TV 0
#define TARGET_OS_WATCH 0
#define TARGET_OS_SIMULATOR 0
#define TARGET_OS_EMBEDDED 0
