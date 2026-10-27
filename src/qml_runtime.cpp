#include "qml_runtime.h"

#include "klickmeister/src/qml_runtime.cxx.h"

#include <QtCore/QByteArray>
#include <QtCore/QCoreApplication>
#include <QtCore/QMetaObject>
#include <QtCore/QPointer>
#include <QtGui/QWindow>

#include <memory>

namespace {

// Accessed only on the Qt thread.
QPointer<QWindow> activationWindow;

bool activateMainWindow(rust::Str token)
{
    QWindow* window = activationWindow;
    // A hidden main window is closing; the later launch then takes over.
    if (!window || !window->isVisible())
        return false;
    // Qt Wayland passes this token to xdg-activation in requestActivate() and
    // clears it there. KDBusService hands launcher tokens over the same way.
    if (token.empty())
        qunsetenv("XDG_ACTIVATION_TOKEN");
    else
        qputenv("XDG_ACTIVATION_TOKEN", QByteArray(token.data(), qsizetype(token.size())));
    const bool activated = QMetaObject::invokeMethod(window, "activateFromLauncher");
    qunsetenv("XDG_ACTIVATION_TOKEN");
    return activated;
}

} // namespace

bool klickmeisterQmlEngineHasRootObject(const QQmlApplicationEngine& engine)
{
    return !engine.rootObjects().isEmpty();
}

void klickmeisterSetActivationWindow(const QQmlApplicationEngine& engine)
{
    const auto roots = engine.rootObjects();
    activationWindow = roots.isEmpty() ? nullptr : qobject_cast<QWindow*>(roots.first());
}

void klickmeisterQueueActivation(rust::Box<Activation> activation)
{
    // Queued functors must be copyable. If the application quits before the
    // functor runs, destroying it drops the request, which reports failure.
    auto pending = std::make_shared<rust::Box<Activation>>(std::move(activation));
    if (auto* application = QCoreApplication::instance()) {
        QMetaObject::invokeMethod(
            application,
            [pending] {
                const bool activated = activateMainWindow(klickmeisterActivationToken(**pending));
                klickmeisterFinishActivation(std::move(*pending), activated);
            },
            Qt::QueuedConnection);
    }
}
