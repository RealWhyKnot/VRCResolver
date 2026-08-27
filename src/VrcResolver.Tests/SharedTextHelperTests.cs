using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

public class SharedTextHelperTests
{
    [Theory]
    [InlineData("https://www.youtube.com/watch?v=abc", "youtube.com")]
    [InlineData("https://youtube.com/watch?v=abc", "youtube.com")]
    [InlineData("https://WWW.Example.COM/x", "example.com")]
    [InlineData("https://wwwx.example.com/x", "wwwx.example.com")]
    [InlineData("not a url", "?")]
    [InlineData("", "?")]
    [InlineData(null, "?")]
    public void BareHost_StripsWwwAndFallsBackToQuestionMark(string? url, string expected)
    {
        Assert.Equal(expected, LogUtil.BareHost(url));
    }

    [Fact]
    public void Base64UrlText_RoundTrips_WithUrlSafeAlphabetAndNoPadding()
    {
        const string value = "https://cdn.example.com/seg?sig=a+b/c=&t=1";
        string encoded = Base64UrlText.Encode(value);
        Assert.DoesNotContain("+", encoded);
        Assert.DoesNotContain("/", encoded);
        Assert.DoesNotContain("=", encoded);
        Assert.True(Base64UrlText.TryDecode(encoded, out string decoded));
        Assert.Equal(value, decoded);
    }

    [Fact]
    public void Base64UrlText_TryDecode_AcceptsPaddedInput()
    {
        Assert.True(Base64UrlText.TryDecode("aGk=", out string decoded));
        Assert.Equal("hi", decoded);
    }

    [Theory]
    [InlineData("!!!!")]
    [InlineData(null)]
    [InlineData("")]
    public void Base64UrlText_TryDecode_RejectsInvalidInput(string? encoded)
    {
        Assert.False(Base64UrlText.TryDecode(encoded, out _));
    }
}
