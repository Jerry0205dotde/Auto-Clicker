#pragma once

#include <QtQml/QQmlApplicationEngine>

#include "rust/cxx.h"

struct Activation;

bool klickmeisterQmlEngineHasRootObject(const QQmlApplicationEngine& engine);
void klickmeisterSetActivationWindow(const QQmlApplicationEngine& engine);
void klickmeisterQueueActivation(rust::Box<Activation> activation);
