"""Monitor identity serialization and portal pre-check of the native starter."""

import tomllib
import unittest

from run import APP_ID, config_text, monitor_identity, registration_problem


class MonitorIdentityTests(unittest.TestCase):
    screen = {
        "name": "eDP-1",
        "manufacturer": "Müller",
        "model": "Panel",
        "serial": "123",
        "width": 1800,
        "height": 1125,
    }

    def test_qml_json_number_format_for_fractional_and_integral_scales(self):
        prefix = '["eDP-1","Müller","Panel","123",1800,1125,'
        cases = [
            (1, "1"),
            (1.0, "1"),
            (1.25, "1.25"),
            (1.5, "1.5"),
            (7 / 6, "1.1666666666666667"),
            (2.0, "2"),
        ]
        for scale, expected in cases:
            with self.subTest(scale=scale):
                screen = {**self.screen, "scale": scale}
                identity = prefix + expected + "]"
                self.assertEqual(monitor_identity(screen), identity)
                # The written config must parse and return the identity unchanged.
                config = tomllib.loads(config_text(screen))
                self.assertEqual(config["monitor_identity"], identity)
                self.assertEqual(config["position_mode"], "fixed")

    def test_toml_escapes_characters_that_qml_json_keeps_raw(self):
        # JSON.stringify keeps DEL and emoji raw; TOML forbids a raw DEL.
        screen = {**self.screen, "name": "eDP\x7f-1", "model": "Panel 🖥", "scale": 1.25}
        identity = '["eDP\x7f-1","Müller","Panel 🖥","123",1800,1125,1.25]'
        self.assertEqual(monitor_identity(screen), identity)
        text = config_text(screen)
        self.assertNotIn("\x7f", text)
        self.assertEqual(tomllib.loads(text)["monitor_identity"], identity)


class FakeDBusError(Exception):
    def __init__(self, name, message):
        super().__init__(message)
        self.name = name

    def get_dbus_name(self):
        return self.name


def raising(error):
    def register():
        raise error

    return register


class RegistrationTests(unittest.TestCase):
    def test_accepted_registration_passes(self):
        self.assertIsNone(registration_problem(lambda: None))

    def test_rejection_names_the_cause_and_the_setup(self):
        # xdg-desktop-portal 1.22.1 with Exec=klickmeister outside its PATH.
        message = f"Could not register app ID: App info not found for '{APP_ID}'"
        problem = registration_problem(
            raising(FakeDBusError("org.freedesktop.portal.Error.Failed", message))
        )
        self.assertIn(message, problem)
        self.assertIn("absolute Exec", problem)
        self.assertIn("Desktop-Integration", problem)

    def test_portals_without_registry_pass(self):
        for name in (
            "org.freedesktop.DBus.Error.ServiceUnknown",
            "org.freedesktop.DBus.Error.UnknownInterface",
            "org.freedesktop.DBus.Error.UnknownMethod",
        ):
            with self.subTest(name=name):
                self.assertIsNone(
                    registration_problem(raising(FakeDBusError(name, "missing")))
                )

    def test_other_errors_are_not_reported_as_rejections(self):
        with self.assertRaises(ModuleNotFoundError):
            registration_problem(raising(ModuleNotFoundError("dbus")))


if __name__ == "__main__":
    unittest.main()
