#pragma once

#include <QtCore/QString>

#include "rust/cxx.h"

void klickmeisterInstallTranslator();
QString klickmeisterTranslate(rust::Str context, rust::Str source);
