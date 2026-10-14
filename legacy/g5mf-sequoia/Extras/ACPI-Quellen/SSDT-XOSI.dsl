DefinitionBlock ("", "SSDT", 2, "ZPSS", "XOSI", 0x00001000)
{
    Method (XOSI, 1, NotSerialized)
    {
        Local0 = Package (0x09)
            {
                "Windows 2001", 
                "Windows 2001 SP1", 
                "Windows 2001.1", 
                "Windows 2001 SP2", 
                "Windows 2006", 
                "Windows 2009", 
                "Windows 2012", 
                "Windows 2013", 
                "Windows 2015"
            }
        If (_OSI ("Darwin"))
        {
            Return ((Match (Local0, MEQ, Arg0, MTR, Zero, Zero) != Ones))
        }
        Else
        {
            Return (_OSI (Arg0))
        }
    }
}

