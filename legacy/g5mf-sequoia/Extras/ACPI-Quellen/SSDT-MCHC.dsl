DefinitionBlock ("", "SSDT", 2, "ZPSS", "MCHC", 0x00000000)
{
    External (_SB_.PC00, DeviceObj)

    Scope (\_SB.PC00)
    {
        Device (MCHC)
        {
            Name (_ADR, Zero)  // _ADR: Address
            Method (_STA, 0, NotSerialized)  // _STA: Status
            {
                If (_OSI ("Darwin"))
                {
                    Return (0x0F)
                }
                Else
                {
                    Return (Zero)
                }
            }
        }
    }
}

