/*
 * SSDT-RTCAWAC: Alder-Lake-Laptops melden die Uhr als AWAC (ACPI000E), die macOS nicht kennt.
 * Unter macOS wird STAS = 1 gesetzt: die Firmware schaltet dann AWAC aus und die klassische RTC (PNP0B00) ein.
 * CondRefOf: fehlt STAS im DSDT, passiert nichts (statt eines ACPI-Fehlers beim Start).
 */
DefinitionBlock ("", "SSDT", 2, "ZPSS", "RTCAWAC", 0x00000000)
{
    External (STAS, IntObj)

    Scope (\)
    {
        Method (_INI, 0, NotSerialized)  // _INI: Initialize
        {
            If (_OSI ("Darwin"))
            {
                If (CondRefOf (\STAS))
                {
                    \STAS = One
                }
            }
        }
    }
}
