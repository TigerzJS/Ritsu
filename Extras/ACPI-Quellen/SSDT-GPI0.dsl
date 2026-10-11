DefinitionBlock ("", "SSDT", 2, "ZPSS", "GPI0", 0x00000000)
{
    External (_SB_.GPI0, DeviceObj)
    External (_SB_.GPI0.XSTA, MethodObj)    // 0 Arguments

    Scope (\_SB.GPI0)
    {
        Method (_STA, 0, NotSerialized)  // _STA: Status
        {
            If (_OSI ("Darwin"))
            {
                Return (0x0F)
            }
            Else
            {
                Return (\_SB.GPI0.XSTA ())
            }
        }
    }
}

