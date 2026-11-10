#include "i18n.h"

#include <QtCore/QByteArray>
#include <QtCore/QCoreApplication>
#include <QtCore/QLocale>
#include <QtCore/QTranslator>

void klickmeisterInstallTranslator()
{
    QCoreApplication* application = QCoreApplication::instance();
    if (!application)
        return;
    auto* translator = new QTranslator(application);
    // The texts are written in English. Follow the user's language order, so
    // English ranked before German keeps the source texts.
    for (const QString& language : QLocale().uiLanguages()) {
        if (language.startsWith(QLatin1String("en")))
            break;
        // Loads klickmeister_de.qm for de-DE as well.
        const QString name = QLatin1String("klickmeister_") + QString(language).replace(QLatin1Char('-'), QLatin1Char('_'));
        if (translator->load(name, QStringLiteral(":/i18n"))) {
            application->installTranslator(translator);
            return;
        }
    }
    delete translator;
}

QString klickmeisterTranslate(rust::Str context, rust::Str source)
{
    const QByteArray contextUtf8(context.data(), qsizetype(context.size()));
    const QByteArray sourceUtf8(source.data(), qsizetype(source.size()));
    return QCoreApplication::translate(contextUtf8.constData(), sourceUtf8.constData());
}
