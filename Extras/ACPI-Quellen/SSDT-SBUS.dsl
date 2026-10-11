DefinitionBlock ("", "SSDT", 2, "ZPSS", "SBUS", 0x00000000)
{
    External (_SB_.PC00.SBUS, DeviceObj)

    Scope (\_SB.PC00.SBUS)
    {
        Device (BUS0)
        {
            Name (_CID, "smbus")  // _CID: Compatible ID
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

